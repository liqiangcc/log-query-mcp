use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

use serde::{Serialize, de::DeserializeOwned};
use thiserror::Error;
use uuid::Uuid;

use crate::CacheConfig;

use super::{
    gc::{GcEntry, GcPlanError, plan_gc},
    generation::{CacheFileId, CacheSourceId, GenerationId, GenerationKey},
    manifest::{
        CacheCatalog, CacheManifest, CatalogFile, CatalogSource, GenerationMetadata,
        GenerationRecord, ManifestValidationError, validate_remote_identifier,
    },
};

const CATALOG_FILE: &str = "catalog.json";
const SOURCES_DIR: &str = "sources";
const MANIFEST_FILE: &str = "manifest.json";
const GENERATIONS_DIR: &str = "generations";
const STAGING_DIR: &str = ".staging";
const MAX_SOURCE_IDENTIFIER_CHARS: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheStoreLimits {
    pub max_bytes: u64,
    pub max_bytes_per_source: u64,
    pub retention: Duration,
    pub max_generations_per_file: usize,
}

impl CacheStoreLimits {
    fn validate(self) -> Result<(), CacheStoreError> {
        if self.max_bytes == 0
            || self.max_bytes_per_source == 0
            || self.max_bytes_per_source > self.max_bytes
            || self.retention.is_zero()
            || self.max_generations_per_file == 0
        {
            return Err(CacheStoreError::InvalidLimits);
        }
        Ok(())
    }

    fn retention_millis(self) -> u64 {
        u64::try_from(self.retention.as_millis()).unwrap_or(u64::MAX)
    }
}

impl From<&CacheConfig> for CacheStoreLimits {
    fn from(config: &CacheConfig) -> Self {
        Self {
            max_bytes: config.max_bytes,
            max_bytes_per_source: config.max_bytes_per_source,
            retention: Duration::from_secs(config.retention_hours.saturating_mul(3600)),
            max_generations_per_file: config.max_generations_per_file,
        }
    }
}

#[derive(Clone)]
pub struct CacheStore {
    inner: Arc<CacheStoreInner>,
}

struct CacheStoreInner {
    root: PathBuf,
    limits: CacheStoreLimits,
    state: Mutex<StoreState>,
}

struct StoreState {
    catalog: CacheCatalog,
    pins: HashMap<GenerationKey, usize>,
}

impl CacheStore {
    pub fn from_config(config: &CacheConfig) -> Result<Self, CacheStoreError> {
        Self::open(&config.root, CacheStoreLimits::from(config))
    }

    pub fn open(root: impl AsRef<Path>, limits: CacheStoreLimits) -> Result<Self, CacheStoreError> {
        limits.validate()?;
        let root = root.as_ref().to_path_buf();
        if !root.exists() {
            fs::create_dir_all(&root)?;
        }
        ensure_private_dir(&root)?;
        ensure_private_dir(&root.join(SOURCES_DIR))?;

        let catalog_path = root.join(CATALOG_FILE);
        let catalog = if catalog_path.exists() {
            let catalog: CacheCatalog = read_json(&catalog_path)?;
            catalog.validate()?;
            catalog
        } else {
            let catalog = CacheCatalog::default();
            save_atomic_json(&catalog_path, &catalog)?;
            catalog
        };

        let store = Self {
            inner: Arc::new(CacheStoreInner {
                root,
                limits,
                state: Mutex::new(StoreState {
                    catalog,
                    pins: HashMap::new(),
                }),
            }),
        };
        store.recover()?;
        Ok(store)
    }

    #[must_use]
    pub fn limits(&self) -> CacheStoreLimits {
        self.inner.limits
    }

    pub fn begin_generation(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<StagedGeneration, CacheStoreError> {
        validate_source_identifier(source_identifier)?;
        validate_remote_identifier(remote_identifier)?;
        let (source_id, file_id) = self.resolve_ids(source_identifier, remote_identifier)?;
        self.ensure_file_layout(&source_id, &file_id)?;

        let generation_id = GenerationId::new();
        let file_dir = self.file_dir(&source_id, &file_id);
        let staging_path = file_dir
            .join(STAGING_DIR)
            .join(format!("{}.tmp", generation_id.as_str()));
        let final_path = file_dir
            .join(GENERATIONS_DIR)
            .join(format!("{}.log", generation_id.as_str()));
        let file = create_private_file(&staging_path)?;

        Ok(StagedGeneration {
            store: self.clone(),
            source_identifier: source_identifier.to_owned(),
            remote_identifier: remote_identifier.to_owned(),
            source_id,
            file_id,
            generation_id,
            staging_path,
            final_path,
            file: Some(file),
            committed: false,
        })
    }

    pub fn begin_append(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<StagedAppend, CacheStoreError> {
        validate_source_identifier(source_identifier)?;
        validate_remote_identifier(remote_identifier)?;
        let Some((source_id, file_id)) = self.lookup_ids(source_identifier, remote_identifier)?
        else {
            return Err(CacheStoreError::GenerationNotFound);
        };
        self.ensure_file_layout(&source_id, &file_id)?;
        let manifest = self
            .load_manifest_by_ids(&source_id, &file_id)?
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let original = manifest
            .current()
            .cloned()
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let staging_path = self
            .file_dir(&source_id, &file_id)
            .join(STAGING_DIR)
            .join(format!(
                "{}-append-{}.tmp",
                original.generation.as_str(),
                Uuid::new_v4().simple()
            ));
        let file = create_private_file(&staging_path)?;

        Ok(StagedAppend {
            store: self.clone(),
            source_id,
            file_id,
            original,
            staging_path,
            file: Some(file),
            committed: false,
        })
    }

    pub fn load_manifest(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<Option<CacheManifest>, CacheStoreError> {
        validate_source_identifier(source_identifier)?;
        validate_remote_identifier(remote_identifier)?;
        let Some((source_id, file_id)) = self.lookup_ids(source_identifier, remote_identifier)?
        else {
            return Ok(None);
        };
        self.load_manifest_by_ids(&source_id, &file_id)
    }

    pub fn pin_current_generation(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<PinnedGeneration, CacheStoreError> {
        let manifest = self
            .load_manifest(source_identifier, remote_identifier)?
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let generation = manifest
            .current_generation
            .clone()
            .ok_or(CacheStoreError::GenerationNotFound)?;
        self.pin_generation(source_identifier, remote_identifier, &generation)
    }

    pub fn lease_generation(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
        generation_id: &GenerationId,
    ) -> Result<GenerationPin, CacheStoreError> {
        validate_source_identifier(source_identifier)?;
        validate_remote_identifier(remote_identifier)?;
        let Some((source_id, file_id)) = self.lookup_ids(source_identifier, remote_identifier)?
        else {
            return Err(CacheStoreError::GenerationNotFound);
        };
        let key = GenerationKey::new(source_id.clone(), file_id.clone(), generation_id.clone());
        let mut state = self.lock_state()?;
        let manifest = self
            .load_manifest_by_ids(&source_id, &file_id)?
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let record = manifest
            .generations
            .iter()
            .find(|record| &record.generation == generation_id)
            .cloned()
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let path = self.generation_path(&key);
        let file = open_regular_private_file(&path)?;
        let actual_len = file.metadata()?.len();
        if actual_len != record.data_len {
            return Err(CacheStoreError::GenerationLengthMismatch {
                expected: record.data_len,
                actual: actual_len,
            });
        }
        *state.pins.entry(key.clone()).or_insert(0) += 1;
        drop(state);

        Ok(GenerationPin {
            inner: Arc::new(GenerationPinInner {
                store: self.clone(),
                key,
                record,
            }),
        })
    }

    pub fn pin_generation(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
        generation_id: &GenerationId,
    ) -> Result<PinnedGeneration, CacheStoreError> {
        validate_source_identifier(source_identifier)?;
        validate_remote_identifier(remote_identifier)?;
        let Some((source_id, file_id)) = self.lookup_ids(source_identifier, remote_identifier)?
        else {
            return Err(CacheStoreError::GenerationNotFound);
        };
        let key = GenerationKey::new(source_id.clone(), file_id.clone(), generation_id.clone());
        let mut state = self.lock_state()?;
        let manifest = self
            .load_manifest_by_ids(&source_id, &file_id)?
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let record = manifest
            .generations
            .iter()
            .find(|record| &record.generation == generation_id)
            .cloned()
            .ok_or(CacheStoreError::GenerationNotFound)?;
        let path = self.generation_path(&key);
        let file = open_regular_private_file(&path)?;
        let actual_len = file.metadata()?.len();
        if actual_len != record.data_len {
            return Err(CacheStoreError::GenerationLengthMismatch {
                expected: record.data_len,
                actual: actual_len,
            });
        }
        *state.pins.entry(key.clone()).or_insert(0) += 1;
        drop(state);

        let limit = record.data_len;
        Ok(PinnedGeneration {
            store: self.clone(),
            key,
            file,
            record,
            position: 0,
            limit,
        })
    }

    pub fn recover(&self) -> Result<RecoveryReport, CacheStoreError> {
        let state = self.lock_state()?;
        state.catalog.validate()?;
        let mut report = RecoveryReport::default();

        for source in &state.catalog.sources {
            for catalog_file in &source.files {
                self.ensure_file_layout(&source.cache_id, &catalog_file.file_id)?;
                let file_dir = self.file_dir(&source.cache_id, &catalog_file.file_id);
                report.orphan_staging_removed +=
                    clean_directory_files(&file_dir.join(STAGING_DIR))?;

                let manifest_path = file_dir.join(MANIFEST_FILE);
                if !manifest_path.exists() {
                    report.orphan_generations_removed +=
                        clean_directory_files(&file_dir.join(GENERATIONS_DIR))?;
                    continue;
                }

                let manifest: CacheManifest = read_json(&manifest_path)?;
                manifest.validate()?;
                if manifest.source_identifier != source.source_identifier
                    || manifest.source_id != source.cache_id
                    || manifest.file_id != catalog_file.file_id
                    || manifest.remote_identifier != catalog_file.remote_identifier
                {
                    return Err(CacheStoreError::ManifestIdentityMismatch);
                }

                let mut referenced = HashMap::new();
                for generation in &manifest.generations {
                    let key = GenerationKey::new(
                        source.cache_id.clone(),
                        catalog_file.file_id.clone(),
                        generation.generation.clone(),
                    );
                    let path = self.generation_path(&key);
                    let file = open_regular_private_file_for_update(&path)?;
                    let actual_len = file.metadata()?.len();
                    if actual_len < generation.data_len {
                        return Err(CacheStoreError::GenerationLengthMismatch {
                            expected: generation.data_len,
                            actual: actual_len,
                        });
                    }
                    if actual_len > generation.data_len {
                        file.set_len(generation.data_len)?;
                        file.sync_all()?;
                        report.repaired_appends += 1;
                    }
                    referenced.insert(path, ());
                    report.generations += 1;
                }
                report.manifests += 1;

                for entry in fs::read_dir(file_dir.join(GENERATIONS_DIR))? {
                    let entry = entry?;
                    let path = entry.path();
                    let metadata = fs::symlink_metadata(&path)?;
                    if metadata.file_type().is_symlink() || !metadata.is_file() {
                        return Err(CacheStoreError::InvalidLayout);
                    }
                    if !referenced.contains_key(&path) {
                        fs::remove_file(path)?;
                        report.orphan_generations_removed += 1;
                    }
                }
            }
        }
        Ok(report)
    }

    pub fn collect_garbage(&self) -> Result<GcReport, CacheStoreError> {
        let state = self.lock_state()?;
        let now = now_unix_millis()?;
        let mut manifests = HashMap::new();
        let mut entries = Vec::new();

        for source in &state.catalog.sources {
            for catalog_file in &source.files {
                let Some(manifest) =
                    self.load_manifest_by_ids(&source.cache_id, &catalog_file.file_id)?
                else {
                    continue;
                };
                let current = manifest.current_generation.as_ref();
                for generation in &manifest.generations {
                    let key = GenerationKey::new(
                        source.cache_id.clone(),
                        catalog_file.file_id.clone(),
                        generation.generation.clone(),
                    );
                    entries.push(GcEntry {
                        key: key.clone(),
                        data_len: generation.data_len,
                        created_at_unix_millis: generation.created_at_unix_millis,
                        is_current: current == Some(&generation.generation),
                        is_pinned: state.pins.get(&key).is_some_and(|count| *count > 0),
                    });
                }
                manifests.insert(
                    (source.cache_id.clone(), catalog_file.file_id.clone()),
                    manifest,
                );
            }
        }

        let plan = plan_gc(
            &entries,
            self.inner.limits.max_bytes,
            self.inner.limits.max_bytes_per_source,
            self.inner.limits.retention_millis(),
            self.inner.limits.max_generations_per_file,
            now,
        )
        .map_err(|error| match error {
            GcPlanError::LimitExceeded => CacheStoreError::CacheLimitExceeded,
        })?;

        let mut grouped: HashMap<(CacheSourceId, CacheFileId), Vec<GenerationId>> = HashMap::new();
        for key in &plan.removals {
            grouped
                .entry((key.source_id.clone(), key.file_id.clone()))
                .or_default()
                .push(key.generation_id.clone());
        }

        for ((source_id, file_id), removals) in grouped {
            let Some(manifest) = manifests.get_mut(&(source_id.clone(), file_id.clone())) else {
                return Err(CacheStoreError::ManifestIdentityMismatch);
            };
            for generation in &removals {
                if !manifest.remove_generation(generation, now) {
                    return Err(CacheStoreError::ProtectedGenerationSelected);
                }
            }
            manifest.validate()?;
            save_atomic_json(&self.manifest_path(&source_id, &file_id), manifest)?;
            for generation in removals {
                let key = GenerationKey::new(source_id.clone(), file_id.clone(), generation);
                let path = self.generation_path(&key);
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        drop(state);

        Ok(GcReport {
            bytes_before: plan.bytes_before,
            bytes_after: plan.bytes_after,
            removed_generations: plan.removals.len(),
            protected_generations: plan.protected_generations,
        })
    }

    fn resolve_ids(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<(CacheSourceId, CacheFileId), CacheStoreError> {
        let mut state = self.lock_state()?;
        let source_index = match state
            .catalog
            .sources
            .iter()
            .position(|source| source.source_identifier == source_identifier)
        {
            Some(index) => index,
            None => {
                state.catalog.sources.push(CatalogSource {
                    source_identifier: source_identifier.to_owned(),
                    cache_id: CacheSourceId::new(),
                    files: Vec::new(),
                });
                state.catalog.sources.len() - 1
            }
        };

        let source = &mut state.catalog.sources[source_index];
        if let Some(file) = source
            .files
            .iter()
            .find(|file| file.remote_identifier == remote_identifier)
        {
            return Ok((source.cache_id.clone(), file.file_id.clone()));
        }

        let file_id = CacheFileId::new();
        source.files.push(CatalogFile {
            remote_identifier: remote_identifier.to_owned(),
            file_id: file_id.clone(),
        });
        let source_id = source.cache_id.clone();
        state.catalog.validate()?;
        save_atomic_json(&self.catalog_path(), &state.catalog)?;
        Ok((source_id, file_id))
    }

    fn lookup_ids(
        &self,
        source_identifier: &str,
        remote_identifier: &str,
    ) -> Result<Option<(CacheSourceId, CacheFileId)>, CacheStoreError> {
        let state = self.lock_state()?;
        let Some(source) = state
            .catalog
            .sources
            .iter()
            .find(|source| source.source_identifier == source_identifier)
        else {
            return Ok(None);
        };
        let Some(file) = source
            .files
            .iter()
            .find(|file| file.remote_identifier == remote_identifier)
        else {
            return Ok(None);
        };
        Ok(Some((source.cache_id.clone(), file.file_id.clone())))
    }

    fn ensure_file_layout(
        &self,
        source_id: &CacheSourceId,
        file_id: &CacheFileId,
    ) -> Result<(), CacheStoreError> {
        let source_dir = self.inner.root.join(SOURCES_DIR).join(source_id.as_str());
        ensure_private_dir(&source_dir)?;
        let file_dir = source_dir.join(file_id.as_str());
        ensure_private_dir(&file_dir)?;
        ensure_private_dir(&file_dir.join(GENERATIONS_DIR))?;
        ensure_private_dir(&file_dir.join(STAGING_DIR))?;
        Ok(())
    }

    fn load_manifest_by_ids(
        &self,
        source_id: &CacheSourceId,
        file_id: &CacheFileId,
    ) -> Result<Option<CacheManifest>, CacheStoreError> {
        let path = self.manifest_path(source_id, file_id);
        if !path.exists() {
            return Ok(None);
        }
        let manifest: CacheManifest = read_json(&path)?;
        manifest.validate()?;
        if manifest.source_id != *source_id || manifest.file_id != *file_id {
            return Err(CacheStoreError::ManifestIdentityMismatch);
        }
        Ok(Some(manifest))
    }

    fn lock_state(&self) -> Result<MutexGuard<'_, StoreState>, CacheStoreError> {
        self.inner
            .state
            .lock()
            .map_err(|_| CacheStoreError::StatePoisoned)
    }

    fn unpin(&self, key: &GenerationKey) {
        let Ok(mut state) = self.inner.state.lock() else {
            return;
        };
        if let Some(count) = state.pins.get_mut(key) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                state.pins.remove(key);
            }
        }
    }

    fn catalog_path(&self) -> PathBuf {
        self.inner.root.join(CATALOG_FILE)
    }

    fn file_dir(&self, source_id: &CacheSourceId, file_id: &CacheFileId) -> PathBuf {
        self.inner
            .root
            .join(SOURCES_DIR)
            .join(source_id.as_str())
            .join(file_id.as_str())
    }

    fn manifest_path(&self, source_id: &CacheSourceId, file_id: &CacheFileId) -> PathBuf {
        self.file_dir(source_id, file_id).join(MANIFEST_FILE)
    }

    fn generation_path(&self, key: &GenerationKey) -> PathBuf {
        self.file_dir(&key.source_id, &key.file_id)
            .join(GENERATIONS_DIR)
            .join(format!("{}.log", key.generation_id.as_str()))
    }
}

pub struct StagedGeneration {
    store: CacheStore,
    source_identifier: String,
    remote_identifier: String,
    source_id: CacheSourceId,
    file_id: CacheFileId,
    generation_id: GenerationId,
    staging_path: PathBuf,
    final_path: PathBuf,
    file: Option<File>,
    committed: bool,
}

impl StagedGeneration {
    #[must_use]
    pub fn generation_id(&self) -> &GenerationId {
        &self.generation_id
    }

    pub fn commit(
        mut self,
        metadata: GenerationMetadata,
    ) -> Result<GenerationRecord, CacheStoreError> {
        metadata.validate()?;
        let mut file = self.file.take().ok_or(CacheStoreError::StagingClosed)?;
        file.flush()?;
        file.sync_all()?;
        let actual_len = file.metadata()?.len();
        let expected_len = metadata.cached_range.len();
        if actual_len != expected_len {
            return Err(CacheStoreError::GenerationLengthMismatch {
                expected: expected_len,
                actual: actual_len,
            });
        }
        drop(file);

        fs::rename(&self.staging_path, &self.final_path)?;
        set_private_file(&self.final_path)?;
        sync_directory(
            self.final_path
                .parent()
                .ok_or(CacheStoreError::InvalidLayout)?,
        )?;

        let now = now_unix_millis()?;
        let record = GenerationRecord {
            generation: self.generation_id.clone(),
            remote_size: metadata.remote_size,
            cached_range: metadata.cached_range,
            remote_mtime_millis: metadata.remote_mtime_millis,
            last_sync_unix_millis: now,
            continuity_fingerprint: metadata.continuity_fingerprint,
            coverage: metadata.coverage,
            data_len: actual_len,
            created_at_unix_millis: now,
        };
        record.validate()?;

        let _state = self.store.lock_state()?;
        let mut manifest = self
            .store
            .load_manifest_by_ids(&self.source_id, &self.file_id)?
            .unwrap_or_else(|| {
                CacheManifest::new(
                    self.source_identifier.clone(),
                    self.source_id.clone(),
                    self.file_id.clone(),
                    self.remote_identifier.clone(),
                    now,
                )
            });
        if manifest.source_identifier != self.source_identifier
            || manifest.remote_identifier != self.remote_identifier
        {
            let _ = fs::remove_file(&self.final_path);
            return Err(CacheStoreError::ManifestIdentityMismatch);
        }
        manifest.append_generation(record.clone(), now);
        manifest.validate()?;
        if let Err(error) = save_atomic_json(
            &self.store.manifest_path(&self.source_id, &self.file_id),
            &manifest,
        ) {
            let _ = fs::remove_file(&self.final_path);
            return Err(error);
        }
        self.committed = true;
        Ok(record)
    }
}

impl Write for StagedGeneration {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "staging file is closed"))?
            .write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "staging file is closed"))?
            .flush()
    }
}

impl Drop for StagedGeneration {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = fs::remove_file(&self.staging_path);
        }
    }
}

pub struct StagedAppend {
    store: CacheStore,
    source_id: CacheSourceId,
    file_id: CacheFileId,
    original: GenerationRecord,
    staging_path: PathBuf,
    file: Option<File>,
    committed: bool,
}

impl StagedAppend {
    #[must_use]
    pub fn generation_id(&self) -> &GenerationId {
        &self.original.generation
    }

    pub fn commit(
        mut self,
        metadata: GenerationMetadata,
    ) -> Result<GenerationRecord, CacheStoreError> {
        metadata.validate()?;
        let mut staging = self.file.take().ok_or(CacheStoreError::StagingClosed)?;
        staging.flush()?;
        staging.sync_all()?;
        let staged_len = staging.metadata()?.len();
        drop(staging);

        let expected_end = self
            .original
            .cached_range
            .end_exclusive
            .checked_add(staged_len)
            .ok_or(CacheStoreError::AppendRangeMismatch)?;
        if metadata.cached_range.start != self.original.cached_range.start
            || metadata.cached_range.end_exclusive != expected_end
        {
            return Err(CacheStoreError::AppendRangeMismatch);
        }

        let _state = self.store.lock_state()?;
        let mut manifest = self
            .store
            .load_manifest_by_ids(&self.source_id, &self.file_id)?
            .ok_or(CacheStoreError::GenerationNotFound)?;
        if manifest.current_generation.as_ref() != Some(&self.original.generation) {
            return Err(CacheStoreError::ConcurrentGenerationChanged);
        }
        let index = manifest
            .generations
            .iter()
            .position(|record| record.generation == self.original.generation)
            .ok_or(CacheStoreError::GenerationNotFound)?;
        if manifest.generations[index] != self.original {
            return Err(CacheStoreError::ConcurrentGenerationChanged);
        }

        let key = GenerationKey::new(
            self.source_id.clone(),
            self.file_id.clone(),
            self.original.generation.clone(),
        );
        let data_path = self.store.generation_path(&key);
        let mut data = open_regular_private_file_for_update(&data_path)?;
        let actual_len = data.metadata()?.len();
        if actual_len < self.original.data_len {
            return Err(CacheStoreError::GenerationLengthMismatch {
                expected: self.original.data_len,
                actual: actual_len,
            });
        }
        if actual_len > self.original.data_len {
            data.set_len(self.original.data_len)?;
            data.sync_all()?;
        }
        data.seek(SeekFrom::Start(self.original.data_len))?;
        let mut staged_reader = File::open(&self.staging_path)?;
        let copied = io::copy(&mut staged_reader, &mut data)?;
        if copied != staged_len {
            data.set_len(self.original.data_len)?;
            data.sync_all()?;
            return Err(CacheStoreError::GenerationLengthMismatch {
                expected: staged_len,
                actual: copied,
            });
        }
        data.flush()?;
        data.sync_all()?;

        let now = now_unix_millis()?;
        let current = &mut manifest.generations[index];
        current.remote_size = metadata.remote_size;
        current.cached_range = metadata.cached_range;
        current.remote_mtime_millis = metadata.remote_mtime_millis;
        current.last_sync_unix_millis = now;
        current.continuity_fingerprint = metadata.continuity_fingerprint;
        current.coverage = metadata.coverage;
        current.data_len = metadata.cached_range.len();
        let updated = current.clone();
        manifest.updated_at_unix_millis = now;
        manifest.validate()?;

        if let Err(error) = save_atomic_json(
            &self.store.manifest_path(&self.source_id, &self.file_id),
            &manifest,
        ) {
            data.set_len(self.original.data_len)?;
            data.sync_all()?;
            return Err(error);
        }

        self.committed = true;
        let _ = fs::remove_file(&self.staging_path);
        if let Some(parent) = self.staging_path.parent() {
            let _ = sync_directory(parent);
        }
        Ok(updated)
    }
}

impl Write for StagedAppend {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "staging file is closed"))?
            .write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "staging file is closed"))?
            .flush()
    }
}

impl Drop for StagedAppend {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = fs::remove_file(&self.staging_path);
        }
    }
}

#[derive(Clone)]
pub struct GenerationPin {
    inner: Arc<GenerationPinInner>,
}

impl std::fmt::Debug for GenerationPin {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GenerationPin")
            .field("generation", &self.inner.key.generation_id)
            .field("data_len", &self.inner.record.data_len)
            .finish()
    }
}

impl PartialEq for GenerationPin {
    fn eq(&self, other: &Self) -> bool {
        self.inner.key == other.inner.key
    }
}

impl Eq for GenerationPin {}

impl GenerationPin {
    #[must_use]
    pub fn generation_id(&self) -> &GenerationId {
        &self.inner.key.generation_id
    }

    #[must_use]
    pub fn record(&self) -> &GenerationRecord {
        &self.inner.record
    }
}

struct GenerationPinInner {
    store: CacheStore,
    key: GenerationKey,
    record: GenerationRecord,
}

impl Drop for GenerationPinInner {
    fn drop(&mut self) {
        self.store.unpin(&self.key);
    }
}

pub struct PinnedGeneration {
    store: CacheStore,
    key: GenerationKey,
    file: File,
    record: GenerationRecord,
    position: u64,
    limit: u64,
}

impl PinnedGeneration {
    #[must_use]
    pub fn record(&self) -> &GenerationRecord {
        &self.record
    }

    #[must_use]
    pub fn generation_id(&self) -> &GenerationId {
        &self.key.generation_id
    }
}

impl Read for PinnedGeneration {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self.limit.saturating_sub(self.position);
        if remaining == 0 || buffer.is_empty() {
            return Ok(0);
        }
        let allowed = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buffer.len());
        let read = self.file.read(&mut buffer[..allowed])?;
        self.position = self
            .position
            .saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        Ok(read)
    }
}

impl Seek for PinnedGeneration {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        let target = match position {
            SeekFrom::Start(offset) => i128::from(offset),
            SeekFrom::End(delta) => i128::from(self.limit) + i128::from(delta),
            SeekFrom::Current(delta) => i128::from(self.position) + i128::from(delta),
        };
        if target < 0 || target > i128::from(self.limit) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek exceeds pinned generation snapshot",
            ));
        }
        let target = u64::try_from(target).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "invalid snapshot seek target")
        })?;
        let actual = self.file.seek(SeekFrom::Start(target))?;
        self.position = actual;
        Ok(actual)
    }
}

impl Drop for PinnedGeneration {
    fn drop(&mut self) {
        self.store.unpin(&self.key);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    pub manifests: usize,
    pub generations: usize,
    pub orphan_staging_removed: usize,
    pub orphan_generations_removed: usize,
    pub repaired_appends: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcReport {
    pub bytes_before: u64,
    pub bytes_after: u64,
    pub removed_generations: usize,
    pub protected_generations: usize,
}

#[derive(Debug, Error)]
pub enum CacheStoreError {
    #[error("cache store I/O failed")]
    Io(#[from] io::Error),
    #[error("cache metadata JSON is invalid")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Manifest(#[from] ManifestValidationError),
    #[error("cache store limits are invalid")]
    InvalidLimits,
    #[error("cache source identifier is invalid")]
    InvalidSourceIdentifier,
    #[error("cache layout contains an unexpected file type")]
    InvalidLayout,
    #[error("cache manifest identity does not match the catalog")]
    ManifestIdentityMismatch,
    #[error("cache generation does not exist")]
    GenerationNotFound,
    #[error("cache generation length mismatch: expected {expected}, actual {actual}")]
    GenerationLengthMismatch { expected: u64, actual: u64 },
    #[error("cache staging file is already closed")]
    StagingClosed,
    #[error("append metadata does not extend the current cached range exactly")]
    AppendRangeMismatch,
    #[error("cache generation changed while append data was staged")]
    ConcurrentGenerationChanged,
    #[error("cache metadata lock is poisoned")]
    StatePoisoned,
    #[error("cache limit exceeded and no safe generation can be collected")]
    CacheLimitExceeded,
    #[error("garbage collection selected a protected generation")]
    ProtectedGenerationSelected,
    #[error("system clock is before the Unix epoch")]
    InvalidSystemTime,
}

fn validate_source_identifier(value: &str) -> Result<(), CacheStoreError> {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(CacheStoreError::InvalidSourceIdentifier);
    };
    if !first.is_ascii_alphanumeric() || value.len() > MAX_SOURCE_IDENTIFIER_CHARS {
        return Err(CacheStoreError::InvalidSourceIdentifier);
    }
    if chars.any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
    }) {
        return Err(CacheStoreError::InvalidSourceIdentifier);
    }
    Ok(())
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, CacheStoreError> {
    let file = open_regular_private_file(path)?;
    Ok(serde_json::from_reader(file)?)
}

fn save_atomic_json<T: Serialize>(path: &Path, value: &T) -> Result<(), CacheStoreError> {
    let parent = path.parent().ok_or(CacheStoreError::InvalidLayout)?;
    ensure_private_dir(parent)?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or(CacheStoreError::InvalidLayout)?;
    let temporary = parent.join(format!(".{file_name}.tmp-{}", Uuid::new_v4().simple()));
    let result = (|| {
        let mut file = create_private_file(&temporary)?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")?;
        file.flush()?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)?;
        set_private_file(path)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_private_dir(path: &Path) -> Result<(), CacheStoreError> {
    if !path.exists() {
        fs::create_dir(path)?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(CacheStoreError::InvalidLayout);
    }
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn create_private_file(path: &Path) -> Result<File, CacheStoreError> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    let file = options.open(path)?;
    set_private_file(path)?;
    Ok(file)
}

fn open_regular_private_file(path: &Path) -> Result<File, CacheStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CacheStoreError::InvalidLayout);
    }
    set_private_file(path)?;
    Ok(File::open(path)?)
}

fn open_regular_private_file_for_update(path: &Path) -> Result<File, CacheStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CacheStoreError::InvalidLayout);
    }
    set_private_file(path)?;
    Ok(OpenOptions::new().read(true).write(true).open(path)?)
}

fn set_private_file(path: &Path) -> Result<(), CacheStoreError> {
    #[cfg(unix)]
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), CacheStoreError> {
    File::open(path)?.sync_all()?;
    Ok(())
}

fn clean_directory_files(path: &Path) -> Result<usize, CacheStoreError> {
    let mut removed = 0;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(CacheStoreError::InvalidLayout);
        }
        fs::remove_file(entry.path())?;
        removed += 1;
    }
    if removed > 0 {
        sync_directory(path)?;
    }
    Ok(removed)
}

fn now_unix_millis() -> Result<u64, CacheStoreError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CacheStoreError::InvalidSystemTime)?;
    Ok(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::manifest::{ByteRange, CacheCoverage};
    use tempfile::TempDir;

    fn limits(max_bytes: u64, max_generations_per_file: usize) -> CacheStoreLimits {
        CacheStoreLimits {
            max_bytes,
            max_bytes_per_source: max_bytes,
            retention: Duration::from_secs(3600),
            max_generations_per_file,
        }
    }

    fn metadata(len: u64) -> GenerationMetadata {
        GenerationMetadata {
            remote_size: len,
            cached_range: ByteRange::new(0, len).expect("range"),
            remote_mtime_millis: Some(1),
            continuity_fingerprint: Some("fingerprint".to_owned()),
            coverage: CacheCoverage::Full,
        }
    }

    fn write_generation(store: &CacheStore, bytes: &[u8]) -> GenerationRecord {
        let mut staged = store
            .begin_generation("service-a", "logs/application.log")
            .expect("stage");
        staged.write_all(bytes).expect("write");
        staged
            .commit(metadata(u64::try_from(bytes.len()).expect("length")))
            .expect("commit")
    }

    #[test]
    fn cache_layout_uses_opaque_ids_and_private_permissions() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
        let record = write_generation(&store, b"abc");
        let manifest = store
            .load_manifest("service-a", "logs/application.log")
            .expect("manifest")
            .expect("present");

        assert!(!manifest.source_id.as_str().contains("service-a"));
        assert!(!manifest.file_id.as_str().contains("application"));
        assert_eq!(manifest.current_generation, Some(record.generation.clone()));
        assert!(
            !store
                .generation_path(&GenerationKey::new(
                    manifest.source_id.clone(),
                    manifest.file_id.clone(),
                    record.generation,
                ))
                .to_string_lossy()
                .contains("application.log")
        );

        #[cfg(unix)]
        {
            let root_mode = fs::metadata(temp.path())
                .expect("root metadata")
                .permissions()
                .mode();
            assert_eq!(root_mode & 0o777, 0o700);
            let manifest_mode =
                fs::metadata(store.manifest_path(&manifest.source_id, &manifest.file_id))
                    .expect("manifest metadata")
                    .permissions()
                    .mode();
            assert_eq!(manifest_mode & 0o777, 0o600);
        }
    }

    #[test]
    fn committed_generation_survives_restart_and_can_be_pinned() {
        let temp = TempDir::new().expect("temp");
        let generation = {
            let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
            write_generation(&store, b"restart-safe").generation
        };

        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("reopen");
        let mut pinned = store
            .pin_generation("service-a", "logs/application.log", &generation)
            .expect("pin");
        let mut text = String::new();
        pinned.read_to_string(&mut text).expect("read");
        assert_eq!(text, "restart-safe");
    }

    #[test]
    fn abandoned_staging_does_not_publish_a_manifest() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
        {
            let mut staged = store
                .begin_generation("service-a", "logs/application.log")
                .expect("stage");
            staged.write_all(b"partial").expect("write");
        }
        assert!(
            store
                .load_manifest("service-a", "logs/application.log")
                .expect("manifest")
                .is_none()
        );
        let report = store.recover().expect("recover");
        assert_eq!(report.generations, 0);
    }

    #[test]
    fn corrupted_manifest_is_detected_on_restart() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
        write_generation(&store, b"abc");
        let manifest = store
            .load_manifest("service-a", "logs/application.log")
            .expect("manifest")
            .expect("present");
        fs::write(
            store.manifest_path(&manifest.source_id, &manifest.file_id),
            b"not-json",
        )
        .expect("corrupt");

        assert!(matches!(
            CacheStore::open(temp.path(), limits(1024, 3)),
            Err(CacheStoreError::Json(_))
        ));
    }

    #[test]
    fn append_keeps_generation_and_pinned_snapshot_length() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
        let first = write_generation(&store, b"abc");
        let mut old_snapshot = store
            .pin_generation("service-a", "logs/application.log", &first.generation)
            .expect("old snapshot");

        let mut append = store
            .begin_append("service-a", "logs/application.log")
            .expect("append");
        assert_eq!(append.generation_id(), &first.generation);
        append.write_all(b"def").expect("append write");
        let updated = append.commit(metadata(6)).expect("append commit");
        assert_eq!(updated.generation, first.generation);
        assert_eq!(updated.data_len, 6);

        let mut old_text = String::new();
        old_snapshot
            .read_to_string(&mut old_text)
            .expect("read old snapshot");
        assert_eq!(old_text, "abc");

        let mut fresh = store
            .pin_current_generation("service-a", "logs/application.log")
            .expect("fresh snapshot");
        let mut fresh_text = String::new();
        fresh.read_to_string(&mut fresh_text).expect("read fresh");
        assert_eq!(fresh_text, "abcdef");
    }

    #[test]
    fn recovery_rolls_back_uncommitted_append_tail() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 3)).expect("store");
        let first = write_generation(&store, b"abc");
        let manifest = store
            .load_manifest("service-a", "logs/application.log")
            .expect("manifest")
            .expect("present");
        let key = GenerationKey::new(
            manifest.source_id.clone(),
            manifest.file_id.clone(),
            first.generation.clone(),
        );
        let path = store.generation_path(&key);
        let mut data = OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open data");
        data.write_all(b"orphan").expect("append orphan");
        data.sync_all().expect("sync orphan");
        drop(data);

        let report = store.recover().expect("recover");
        assert_eq!(report.repaired_appends, 1);
        let mut pinned = store
            .pin_current_generation("service-a", "logs/application.log")
            .expect("pin");
        let mut text = String::new();
        pinned.read_to_string(&mut text).expect("read");
        assert_eq!(text, "abc");
    }

    #[test]
    fn pinned_old_generation_is_not_removed_by_gc() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(1024, 2)).expect("store");
        let first = write_generation(&store, b"one");
        let second = write_generation(&store, b"two");
        let third = write_generation(&store, b"three");
        let pin = store
            .pin_generation("service-a", "logs/application.log", &first.generation)
            .expect("pin");

        let report = store.collect_garbage().expect("gc");
        assert_eq!(report.removed_generations, 1);
        let manifest = store
            .load_manifest("service-a", "logs/application.log")
            .expect("manifest")
            .expect("present");
        assert!(
            manifest
                .generations
                .iter()
                .any(|record| record.generation == first.generation)
        );
        assert!(
            !manifest
                .generations
                .iter()
                .any(|record| record.generation == second.generation)
        );
        assert_eq!(manifest.current_generation, Some(third.generation));
        drop(pin);
    }

    #[test]
    fn quota_failure_is_stable_when_only_current_generation_remains() {
        let temp = TempDir::new().expect("temp");
        let store = CacheStore::open(temp.path(), limits(2, 2)).expect("store");
        write_generation(&store, b"abc");
        assert!(matches!(
            store.collect_garbage(),
            Err(CacheStoreError::CacheLimitExceeded)
        ));
    }
}
