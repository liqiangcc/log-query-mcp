from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    return text.replace(old, new)


def patch(path: str, edits):
    file = Path(path)
    text = file.read_text()
    for old, new, label in edits:
        text = replace_once(text, old, new, label)
    file.write_text(text)


# Cache generation lease: cursor snapshots can protect generations without owning a reader.
patch(
    "src/cache/store.rs",
    [
        (
            '''    pub fn pin_generation(\n        &self,\n        source_identifier: &str,\n        remote_identifier: &str,\n        generation_id: &GenerationId,\n    ) -> Result<PinnedGeneration, CacheStoreError> {''',
            '''    pub fn lease_generation(\n        &self,\n        source_identifier: &str,\n        remote_identifier: &str,\n        generation_id: &GenerationId,\n    ) -> Result<GenerationPin, CacheStoreError> {\n        validate_source_identifier(source_identifier)?;\n        validate_remote_identifier(remote_identifier)?;\n        let Some((source_id, file_id)) = self.lookup_ids(source_identifier, remote_identifier)?\n        else {\n            return Err(CacheStoreError::GenerationNotFound);\n        };\n        let key = GenerationKey::new(source_id.clone(), file_id.clone(), generation_id.clone());\n        let mut state = self.lock_state()?;\n        let manifest = self\n            .load_manifest_by_ids(&source_id, &file_id)?\n            .ok_or(CacheStoreError::GenerationNotFound)?;\n        let record = manifest\n            .generations\n            .iter()\n            .find(|record| &record.generation == generation_id)\n            .cloned()\n            .ok_or(CacheStoreError::GenerationNotFound)?;\n        let path = self.generation_path(&key);\n        let file = open_regular_private_file(&path)?;\n        let actual_len = file.metadata()?.len();\n        if actual_len != record.data_len {\n            return Err(CacheStoreError::GenerationLengthMismatch {\n                expected: record.data_len,\n                actual: actual_len,\n            });\n        }\n        *state.pins.entry(key.clone()).or_insert(0) += 1;\n        drop(state);\n\n        Ok(GenerationPin {\n            inner: Arc::new(GenerationPinInner {\n                store: self.clone(),\n                key,\n                record,\n            }),\n        })\n    }\n\n    pub fn pin_generation(\n        &self,\n        source_identifier: &str,\n        remote_identifier: &str,\n        generation_id: &GenerationId,\n    ) -> Result<PinnedGeneration, CacheStoreError> {''',
            "lease_generation",
        ),
        (
            '''pub struct PinnedGeneration {\n    store: CacheStore,''',
            '''#[derive(Clone)]\npub struct GenerationPin {\n    inner: Arc<GenerationPinInner>,\n}\n\nimpl std::fmt::Debug for GenerationPin {\n    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        formatter\n            .debug_struct("GenerationPin")\n            .field("generation", &self.inner.key.generation_id)\n            .field("data_len", &self.inner.record.data_len)\n            .finish()\n    }\n}\n\nimpl PartialEq for GenerationPin {\n    fn eq(&self, other: &Self) -> bool {\n        self.inner.key == other.inner.key\n    }\n}\n\nimpl Eq for GenerationPin {}\n\nimpl GenerationPin {\n    #[must_use]\n    pub fn generation_id(&self) -> &GenerationId {\n        &self.inner.key.generation_id\n    }\n\n    #[must_use]\n    pub fn record(&self) -> &GenerationRecord {\n        &self.inner.record\n    }\n}\n\nstruct GenerationPinInner {\n    store: CacheStore,\n    key: GenerationKey,\n    record: GenerationRecord,\n}\n\nimpl Drop for GenerationPinInner {\n    fn drop(&mut self) {\n        self.store.unpin(&self.key);\n    }\n}\n\npub struct PinnedGeneration {\n    store: CacheStore,''',
            "GenerationPin type",
        ),
    ],
)

patch(
    "src/cache/mod.rs",
    [(
        '''    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,\n    StagedAppend, StagedGeneration,''',
        '''    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, GenerationPin, PinnedGeneration,\n    RecoveryReport, StagedAppend, StagedGeneration,''',
        "cache export pin",
    )],
)

patch(
    "src/lib.rs",
    [(
        '''    CacheStoreLimits, GcReport, GenerationId, GenerationKey, GenerationMetadata, GenerationRecord,\n    ManifestValidationError, PinnedGeneration, RecoveryReport, RemoteSyncTarget, StagedAppend,''',
        '''    CacheStoreLimits, GcReport, GenerationId, GenerationKey, GenerationMetadata, GenerationPin,\n    GenerationRecord, ManifestValidationError, PinnedGeneration, RecoveryReport, RemoteSyncTarget, StagedAppend,''',
        "lib export pin",
    )],
)

# Share a single SSH connection manager between discovery and synchronization.
patch(
    "src/cache/sync.rs",
    [(
        '''impl SyncEngine {\n    pub fn from_config(config: &AppConfigV2, cache: CacheStore) -> Result<Self, SyncError> {\n        if config.limits.max_sync_bytes_per_query == 0 {\n            return Err(SyncError::InvalidConfiguration);\n        }\n        Ok(Self {\n            cache,\n            connections: SshConnectionManager::from_config(config)?,\n            max_sync_bytes_per_query: config.limits.max_sync_bytes_per_query,\n        })\n    }''',
        '''impl SyncEngine {\n    pub fn from_config(config: &AppConfigV2, cache: CacheStore) -> Result<Self, SyncError> {\n        Self::new(\n            cache,\n            SshConnectionManager::from_config(config)?,\n            config.limits.max_sync_bytes_per_query,\n        )\n    }\n\n    pub fn new(\n        cache: CacheStore,\n        connections: SshConnectionManager,\n        max_sync_bytes_per_query: u64,\n    ) -> Result<Self, SyncError> {\n        if max_sync_bytes_per_query == 0 {\n            return Err(SyncError::InvalidConfiguration);\n        }\n        Ok(Self {\n            cache,\n            connections,\n            max_sync_bytes_per_query,\n        })\n    }''',
        "shared sync manager",
    )],
)

# Backend abstraction now returns a generic local/cache snapshot reader.
Path("src/backend/mod.rs").write_text(r'''use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::{
    FileIdentity, GenerationPin, PinnedGeneration, SafeFile, SourceRegistryError,
};

mod local;
mod remote;

pub(crate) use local::LocalBackend;
pub(crate) use remote::RemoteBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendFileSnapshot {
    pub(crate) relative_path: std::path::PathBuf,
    pub(crate) identity: FileIdentity,
    pub(crate) size_at_snapshot: u64,
    pub(crate) coverage: Option<crate::CacheCoverage>,
    pub(crate) generation_pin: Option<GenerationPin>,
}

#[derive(Debug)]
pub enum SnapshotFile {
    Local(SafeFile),
    Remote {
        reader: PinnedGeneration,
        identity: FileIdentity,
        size: u64,
    },
}

impl SnapshotFile {
    #[must_use]
    pub fn identity(&self) -> FileIdentity {
        match self {
            Self::Local(file) => file.identity(),
            Self::Remote { identity, .. } => *identity,
        }
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        match self {
            Self::Local(file) => file.size(),
            Self::Remote { size, .. } => *size,
        }
    }
}

impl Read for SnapshotFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(file) => file.file().read(buffer),
            Self::Remote { reader, .. } => reader.read(buffer),
        }
    }
}

impl Seek for SnapshotFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Local(file) => file.file().seek(position),
            Self::Remote { reader, .. } => reader.seek(position),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SourceBackend {
    Local(LocalBackend),
    Remote(RemoteBackend),
}

impl SourceBackend {
    pub(crate) fn startup_validate(&self, source_id: &str) -> Result<(), SourceRegistryError> {
        match self {
            Self::Local(backend) => {
                backend.snapshot_files(source_id, crate::MAX_REGISTERED_FILES_PER_SOURCE)?;
                Ok(())
            }
            Self::Remote(_) => Ok(()),
        }
    }

    pub(crate) fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.snapshot_files(source_id, max_files),
            Self::Remote(_) => Err(SourceRegistryError::AsyncBackendRequired),
        }
    }

    pub(crate) async fn query_snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.snapshot_files(source_id, max_files),
            Self::Remote(backend) => backend.snapshot_files(source_id, max_files).await,
        }
    }

    pub(crate) fn open_snapshot_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_snapshot: u64,
        file_id: &str,
        generation_pin: Option<&GenerationPin>,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_snapshot_file(
                source_id,
                relative_path,
                identity,
                size_at_snapshot,
                file_id,
            ),
            Self::Remote(backend) => backend.open_snapshot_file(
                source_id,
                relative_path,
                identity,
                size_at_snapshot,
                file_id,
                generation_pin,
            ),
        }
    }

    pub(crate) fn open_referenced_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_match: u64,
        file_id: &str,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_referenced_file(
                source_id,
                relative_path,
                identity,
                size_at_match,
                file_id,
            ),
            Self::Remote(backend) => backend.open_referenced_file(
                source_id,
                relative_path,
                identity,
                size_at_match,
                file_id,
            ),
        }
    }

    pub(crate) fn open_configured_file(
        &self,
        source_id: &str,
        relative_path: &Path,
    ) -> Result<SafeFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_configured_file(source_id, relative_path),
            Self::Remote(_) => Err(SourceRegistryError::AsyncBackendRequired),
        }
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        match self {
            Self::Local(backend) => backend.path_is_configured(relative_path),
            Self::Remote(backend) => backend.path_is_configured(relative_path),
        }
    }
}
''')

# Local backend wraps SafeFile in SnapshotFile and adds reference validation.
patch(
    "src/backend/local.rs",
    [
        (
            '''use super::BackendFileSnapshot;''',
            '''use super::{BackendFileSnapshot, SnapshotFile};''',
            "local snapshot import",
        ),
        (
            '''                |(relative_path, (identity, size_at_snapshot))| BackendFileSnapshot {\n                    relative_path,\n                    identity,\n                    size_at_snapshot,\n                },''',
            '''                |(relative_path, (identity, size_at_snapshot))| BackendFileSnapshot {\n                    relative_path,\n                    identity,\n                    size_at_snapshot,\n                    coverage: None,\n                    generation_pin: None,\n                },''',
            "local snapshot fields",
        ),
        (
            ''') -> Result<SafeFile, SourceRegistryError> {\n        if !self.path_is_configured(relative_path) {''',
            ''') -> Result<SnapshotFile, SourceRegistryError> {\n        if !self.path_is_configured(relative_path) {''',
            "local snapshot return",
        ),
        (
            '''        Ok(file)\n    }\n\n    pub(crate) fn open_configured_file(''',
            '''        Ok(SnapshotFile::Local(file))\n    }\n\n    pub(crate) fn open_referenced_file(\n        &self,\n        source_id: &str,\n        relative_path: &Path,\n        identity: FileIdentity,\n        size_at_match: u64,\n        file_id: &str,\n    ) -> Result<SnapshotFile, SourceRegistryError> {\n        if !self.path_is_configured(relative_path) {\n            return Err(SourceRegistryError::PathNotConfigured);\n        }\n        let file = self\n            .root\n            .open_regular_file(relative_path)\n            .map_err(|source| SourceRegistryError::FileUnavailable {\n                source_id: source_id.to_owned(),\n                source,\n            })?;\n        if file.identity() != identity || file.size() < size_at_match {\n            return Err(SourceRegistryError::FileChanged {\n                source_id: source_id.to_owned(),\n                file_id: file_id.to_owned(),\n            });\n        }\n        Ok(SnapshotFile::Local(file))\n    }\n\n    pub(crate) fn open_configured_file(''',
            "local referenced reader",
        ),
    ],
)

# Remote backend: SFTP discovery -> SyncEngine -> CacheStore generation pin.
Path("src/backend/remote.rs").write_text(r'''use std::{
    collections::BTreeMap,
    fmt,
    path::{Component, Path, PathBuf},
};

use tokio::task::JoinSet;
use uuid::Uuid;

use crate::{
    CacheStore, FileIdentity, GenerationId, GenerationPin, LogSourceConfigV2, RemoteFileType,
    RemoteSyncTarget, SourceRegistryError, SshConnectionManager, SyncEngine,
};

use super::{BackendFileSnapshot, SnapshotFile};

pub(crate) struct RemoteBackend {
    source: LogSourceConfigV2,
    cache: CacheStore,
    sync: SyncEngine,
    connections: SshConnectionManager,
    max_remote_files: usize,
}

impl fmt::Debug for RemoteBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteBackend")
            .field("source_id", &self.source.source_id)
            .field("max_remote_files", &self.max_remote_files)
            .finish_non_exhaustive()
    }
}

impl RemoteBackend {
    pub(crate) fn new(
        source: LogSourceConfigV2,
        cache: CacheStore,
        sync: SyncEngine,
        connections: SshConnectionManager,
        max_remote_files: usize,
    ) -> Result<Self, SourceRegistryError> {
        if max_remote_files == 0 {
            return Err(SourceRegistryError::RemoteConfigurationInvalid);
        }
        if source.directories.iter().any(|rule| rule.recursive) {
            return Err(SourceRegistryError::RemoteRecursiveDiscoveryUnsupported {
                source_id: source.source_id.clone(),
            });
        }
        Ok(Self {
            source,
            cache,
            sync,
            connections,
            max_remote_files,
        })
    }

    pub(crate) async fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        let limit = max_files.min(self.max_remote_files);
        if limit == 0 {
            return Err(SourceRegistryError::TooManyFiles {
                source_id: source_id.to_owned(),
                limit,
            });
        }
        let identifiers = self.discover_identifiers(source_id, limit).await?;
        let mut tasks = JoinSet::new();
        for identifier in identifiers {
            let sync = self.sync.clone();
            let source = self.source.clone();
            tasks.spawn(async move {
                let target = RemoteSyncTarget::from_source(&source, identifier.clone())?;
                let outcome = sync.sync(&target).await?;
                Ok::<_, crate::SyncError>((identifier, outcome))
            });
        }

        let mut synchronized = BTreeMap::new();
        while let Some(joined) = tasks.join_next().await {
            let (identifier, outcome) = match joined {
                Ok(Ok(value)) => value,
                Ok(Err(source)) => {
                    tasks.abort_all();
                    return Err(SourceRegistryError::RemoteSync {
                        source_id: source_id.to_owned(),
                        source,
                    });
                }
                Err(source) => {
                    tasks.abort_all();
                    return Err(SourceRegistryError::RemoteTaskJoin {
                        source_id: source_id.to_owned(),
                        source,
                    });
                }
            };
            synchronized.insert(identifier, outcome);
        }

        synchronized
            .into_iter()
            .enumerate()
            .map(|(index, (identifier, outcome))| {
                let pin = self
                    .cache
                    .lease_generation(source_id, &identifier, &outcome.generation)
                    .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                        source_id: source_id.to_owned(),
                        file_id: format!("remote_{index}"),
                        source,
                    })?;
                let relative_path = PathBuf::from(&identifier);
                Ok(BackendFileSnapshot {
                    relative_path,
                    identity: generation_identity(pin.generation_id())?,
                    size_at_snapshot: outcome.cached_range.len(),
                    coverage: Some(outcome.coverage),
                    generation_pin: Some(pin),
                })
            })
            .collect()
    }

    pub(crate) fn open_snapshot_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_snapshot: u64,
        file_id: &str,
        generation_pin: Option<&GenerationPin>,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }
        let pin = generation_pin.ok_or(SourceRegistryError::RemoteSnapshotMissingPin)?;
        if generation_identity(pin.generation_id())? != identity || pin.record().data_len != size_at_snapshot {
            return Err(SourceRegistryError::FileChanged {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
            });
        }
        let identifier = remote_identifier(relative_path)?;
        let reader = self
            .cache
            .pin_generation(source_id, &identifier, pin.generation_id())
            .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
                source,
            })?;
        Ok(SnapshotFile::Remote {
            reader,
            identity,
            size: size_at_snapshot,
        })
    }

    pub(crate) fn open_referenced_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_match: u64,
        file_id: &str,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }
        let generation = identity_generation(identity)?;
        let identifier = remote_identifier(relative_path)?;
        let reader = self
            .cache
            .pin_generation(source_id, &identifier, &generation)
            .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
                source,
            })?;
        if reader.record().data_len < size_at_match {
            return Err(SourceRegistryError::FileChanged {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
            });
        }
        let size = reader.record().data_len;
        Ok(SnapshotFile::Remote {
            reader,
            identity,
            size,
        })
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        self.source.files.iter().any(|candidate| candidate == relative_path)
            || self
                .source
                .directories
                .iter()
                .any(|rule| directory_rule_allows(rule, relative_path))
    }

    async fn discover_identifiers(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<String>, SourceRegistryError> {
        let connection_id = self
            .source
            .backend
            .connection_id
            .as_deref()
            .ok_or(SourceRegistryError::RemoteConfigurationInvalid)?;
        let reader = self
            .connections
            .open_reader(connection_id)
            .await
            .map_err(|source| SourceRegistryError::RemoteTransport {
                source_id: source_id.to_owned(),
                source,
            })?;
        let result = self.discover_with_reader(source_id, max_files, &reader).await;
        let _ = reader.close().await;
        result
    }

    async fn discover_with_reader(
        &self,
        source_id: &str,
        max_files: usize,
        reader: &crate::SshReadTransport,
    ) -> Result<Vec<String>, SourceRegistryError> {
        let mut candidates = BTreeMap::<String, ()>::new();
        for (index, path) in self.source.files.iter().enumerate() {
            let identifier = remote_identifier(path)?;
            let remote_path = configured_remote_path(&self.source.root, path)?;
            let metadata = reader
                .lstat(&remote_path)
                .await
                .map_err(|source| SourceRegistryError::RemoteTransport {
                    source_id: source_id.to_owned(),
                    source,
                })?;
            if metadata.file_type != RemoteFileType::Regular {
                return Err(SourceRegistryError::RemoteExplicitFileNotRegular {
                    source_id: source_id.to_owned(),
                    file_index: index,
                });
            }
            candidates.insert(identifier, ());
            ensure_file_limit(source_id, candidates.len(), max_files)?;
        }

        for rule in &self.source.directories {
            let directory = configured_remote_path(&self.source.root, &rule.path)?;
            let entries = reader
                .read_dir(&directory)
                .await
                .map_err(|source| SourceRegistryError::RemoteTransport {
                    source_id: source_id.to_owned(),
                    source,
                })?;
            if entries.len() > self.max_remote_files {
                return Err(SourceRegistryError::TooManyFiles {
                    source_id: source_id.to_owned(),
                    limit: self.max_remote_files,
                });
            }
            for entry in entries {
                if !valid_remote_name(&entry.file_name)
                    || !rule
                        .include_suffixes
                        .iter()
                        .any(|suffix| entry.file_name.ends_with(suffix))
                {
                    continue;
                }
                let relative_path = if rule.path == Path::new(".") {
                    PathBuf::from(&entry.file_name)
                } else {
                    rule.path.join(&entry.file_name)
                };
                let remote_path = configured_remote_path(&self.source.root, &relative_path)?;
                let metadata = reader
                    .lstat(&remote_path)
                    .await
                    .map_err(|source| SourceRegistryError::RemoteTransport {
                        source_id: source_id.to_owned(),
                        source,
                    })?;
                if metadata.file_type != RemoteFileType::Regular {
                    continue;
                }
                candidates.insert(remote_identifier(&relative_path)?, ());
                ensure_file_limit(source_id, candidates.len(), max_files)?;
            }
        }
        Ok(candidates.into_keys().collect())
    }
}

fn ensure_file_limit(
    source_id: &str,
    count: usize,
    limit: usize,
) -> Result<(), SourceRegistryError> {
    if count > limit {
        Err(SourceRegistryError::TooManyFiles {
            source_id: source_id.to_owned(),
            limit,
        })
    } else {
        Ok(())
    }
}

fn valid_remote_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

fn remote_identifier(path: &Path) -> Result<String, SourceRegistryError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or(SourceRegistryError::RemotePathInvalid)?;
                if !valid_remote_name(value) {
                    return Err(SourceRegistryError::RemotePathInvalid);
                }
                parts.push(value);
            }
            _ => return Err(SourceRegistryError::RemotePathInvalid),
        }
    }
    if parts.is_empty() {
        return Err(SourceRegistryError::RemotePathInvalid);
    }
    Ok(parts.join("/"))
}

fn configured_remote_path(root: &Path, relative: &Path) -> Result<String, SourceRegistryError> {
    let root = root.to_str().ok_or(SourceRegistryError::RemotePathInvalid)?;
    if root.is_empty() || !root.starts_with('/') || root.chars().any(char::is_control) {
        return Err(SourceRegistryError::RemotePathInvalid);
    }
    let identifier = if relative == Path::new(".") {
        String::new()
    } else {
        remote_identifier(relative)?
    };
    let root = if root == "/" { "" } else { root.trim_end_matches('/') };
    if identifier.is_empty() {
        Ok(if root.is_empty() { "/".to_owned() } else { root.to_owned() })
    } else {
        Ok(format!("{root}/{identifier}"))
    }
}

fn directory_rule_allows(rule: &crate::DirectoryRule, relative_path: &Path) -> bool {
    let Some(file_name) = relative_path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if !rule
        .include_suffixes
        .iter()
        .any(|suffix| file_name.ends_with(suffix))
    {
        return false;
    }
    let remainder = if rule.path == Path::new(".") {
        relative_path
    } else {
        let Ok(remainder) = relative_path.strip_prefix(&rule.path) else {
            return false;
        };
        remainder
    };
    remainder.components().count() == 1
}

fn generation_identity(generation: &GenerationId) -> Result<FileIdentity, SourceRegistryError> {
    let uuid = Uuid::parse_str(generation.as_str()).map_err(|_| SourceRegistryError::RemotePathInvalid)?;
    let value = uuid.as_u128();
    Ok(FileIdentity {
        device: (value >> 64) as u64,
        inode: value as u64,
    })
}

fn identity_generation(identity: FileIdentity) -> Result<GenerationId, SourceRegistryError> {
    let value = (u128::from(identity.device) << 64) | u128::from(identity.inode);
    let uuid = Uuid::from_u128(value);
    GenerationId::parse(uuid.simple().to_string()).map_err(|_| SourceRegistryError::RemotePathInvalid)
}
''')

# Source registry supports remote runtime and keeps cache pin in snapshots.
patch(
    "src/source_registry.rs",
    [
        (
            '''    AppConfig, AppConfigV2, BackendType, ConfigDocument, ConfigV2ValidationError,\n    ConfigValidationError, FileIdentity, LimitsConfig, SafeFile, SafeOpenError,\n    SourceDiscoveryError, TimestampRule,\n    backend::{LocalBackend, SourceBackend},''',
            '''    AppConfig, AppConfigV2, BackendType, CacheCoverage, CacheStore, CacheStoreError,\n    ConfigDocument, ConfigV2ValidationError, ConfigValidationError, FileIdentity, GenerationPin,\n    LimitsConfig, SafeFile, SafeOpenError, SourceDiscoveryError, SshConnectionManager,\n    SshTransportError, SyncEngine, SyncError, TimestampRule,\n    backend::{LocalBackend, RemoteBackend, SnapshotFile, SourceBackend},''',
            "registry imports",
        ),
        (
            '''    size_at_snapshot: u64,\n}''',
            '''    size_at_snapshot: u64,\n    coverage: Option<CacheCoverage>,\n    generation_pin: Option<GenerationPin>,\n}''',
            "snapshot remote fields",
        ),
        (
            '''    pub const fn size_at_snapshot(&self) -> u64 {\n        self.size_at_snapshot\n    }''',
            '''    pub const fn size_at_snapshot(&self) -> u64 {\n        self.size_at_snapshot\n    }\n\n    #[must_use]\n    pub fn coverage(&self) -> Option<&CacheCoverage> {\n        self.coverage.as_ref()\n    }\n\n    #[must_use]\n    pub fn generation_pin(&self) -> Option<&GenerationPin> {\n        self.generation_pin.as_ref()\n    }''',
            "snapshot accessors",
        ),
        (
            '''                identity: snapshot.identity,\n                size_at_snapshot: snapshot.size_at_snapshot,\n            })''',
            '''                identity: snapshot.identity,\n                size_at_snapshot: snapshot.size_at_snapshot,\n                coverage: snapshot.coverage,\n                generation_pin: snapshot.generation_pin,\n            })''',
            "snapshot mapping",
        ),
        (
            '''    pub fn open_snapshot_file(\n        &self,\n        snapshot: &SourceFileSnapshot,\n    ) -> Result<SafeFile, SourceRegistryError> {''',
            '''    pub async fn query_snapshot_files(\n        &self,\n        max_files: usize,\n    ) -> Result<Vec<SourceFileSnapshot>, SourceRegistryError> {\n        if max_files == 0 || max_files > MAX_REGISTERED_FILES_PER_SOURCE {\n            return Err(SourceRegistryError::TooManyFiles {\n                source_id: self.descriptor.source_id.clone(),\n                limit: max_files,\n            });\n        }\n        let snapshots = self\n            .backend\n            .query_snapshot_files(&self.descriptor.source_id, max_files)\n            .await?;\n        Ok(snapshots\n            .into_iter()\n            .enumerate()\n            .map(|(index, snapshot)| SourceFileSnapshot {\n                source_id: self.descriptor.source_id.clone(),\n                file_id: stable_file_id(&self.descriptor.source_id, &snapshot.relative_path, index),\n                relative_path: snapshot.relative_path,\n                identity: snapshot.identity,\n                size_at_snapshot: snapshot.size_at_snapshot,\n                coverage: snapshot.coverage,\n                generation_pin: snapshot.generation_pin,\n            })\n            .collect())\n    }\n\n    pub fn open_snapshot_file(\n        &self,\n        snapshot: &SourceFileSnapshot,\n    ) -> Result<SnapshotFile, SourceRegistryError> {''',
            "async query snapshots",
        ),
        (
            '''            snapshot.size_at_snapshot,\n            &snapshot.file_id,\n        )''',
            '''            snapshot.size_at_snapshot,\n            &snapshot.file_id,\n            snapshot.generation_pin.as_ref(),\n        )''',
            "open snapshot pin",
        ),
        (
            '''    pub fn open_configured_file(\n        &self,\n        relative_path: impl AsRef<Path>,\n    ) -> Result<SafeFile, SourceRegistryError> {''',
            '''    pub fn open_referenced_file(\n        &self,\n        relative_path: impl AsRef<Path>,\n        identity: FileIdentity,\n        size_at_match: u64,\n        file_id: &str,\n    ) -> Result<SnapshotFile, SourceRegistryError> {\n        self.backend.open_referenced_file(\n            &self.descriptor.source_id,\n            relative_path.as_ref(),\n            identity,\n            size_at_match,\n            file_id,\n        )\n    }\n\n    pub fn open_configured_file(\n        &self,\n        relative_path: impl AsRef<Path>,\n    ) -> Result<SafeFile, SourceRegistryError> {''',
            "referenced file API",
        ),
        (
            '''    pub fn from_config_v2(config: AppConfigV2) -> Result<Self, SourceRegistryError> {\n        config.validate()?;\n        if let Some(source) = config\n            .sources\n            .iter()\n            .find(|source| source.enabled && source.backend.backend_type == BackendType::Ssh)\n        {\n            return Err(SourceRegistryError::BackendUnavailable {\n                source_id: source.source_id.clone(),\n                backend: "ssh",\n            });\n        }\n        Self::from_config(config.as_v1_shape())\n    }''',
            '''    pub fn from_config_v2(config: AppConfigV2) -> Result<Self, SourceRegistryError> {\n        config.validate()?;\n        let limits = config.limits.local_limits();\n        let has_remote = config\n            .sources\n            .iter()\n            .any(|source| source.enabled && source.backend.backend_type == BackendType::Ssh);\n        let remote_runtime = if has_remote {\n            let cache_config = config\n                .cache\n                .as_ref()\n                .ok_or(SourceRegistryError::RemoteConfigurationInvalid)?;\n            let cache = CacheStore::from_config(cache_config)\n                .map_err(SourceRegistryError::CacheInitialization)?;\n            let connections = SshConnectionManager::from_config(&config)\n                .map_err(SourceRegistryError::TransportInitialization)?;\n            let sync = SyncEngine::new(\n                cache.clone(),\n                connections.clone(),\n                config.limits.max_sync_bytes_per_query,\n            )\n            .map_err(SourceRegistryError::SyncInitialization)?;\n            Some((cache, connections, sync))\n        } else {\n            None\n        };\n\n        let mut sources = Vec::new();\n        let mut by_id = HashMap::new();\n        for source_config in config.sources.into_iter().filter(|source| source.enabled) {\n            let source_id = source_config.source_id.clone();\n            let backend = match source_config.backend.backend_type {\n                BackendType::Local => SourceBackend::Local(LocalBackend::from_config(\n                    &source_id,\n                    &source_config.to_v1_config(),\n                )?),\n                BackendType::Ssh => {\n                    let (cache, connections, sync) = remote_runtime\n                        .as_ref()\n                        .ok_or(SourceRegistryError::RemoteConfigurationInvalid)?;\n                    SourceBackend::Remote(RemoteBackend::new(\n                        source_config.clone(),\n                        cache.clone(),\n                        sync.clone(),\n                        connections.clone(),\n                        config.limits.max_remote_files_per_source,\n                    )?)\n                }\n            };\n            let configured = Arc::new(ConfiguredSource {\n                descriptor: SourceDescriptor {\n                    source_id: source_config.source_id.clone(),\n                    name: source_config.name,\n                    description: source_config.description,\n                    service: source_config.service,\n                    environment: source_config.environment,\n                    tags: source_config.tags,\n                },\n                backend,\n                timestamp_rule: source_config.timestamp_rule,\n            });\n            configured.backend.startup_validate(&source_id)?;\n            let index = sources.len();\n            by_id.insert(source_id, index);\n            sources.push(configured);\n        }\n        Ok(Self {\n            sources,\n            by_id,\n            limits,\n        })\n    }''',
            "v2 remote registry",
        ),
        (
            '''            // Fail startup for unsafe explicit files, invalid directory roots,\n            // or a discovery result beyond the absolute v1 hard limit.\n            configured.snapshot_files(MAX_REGISTERED_FILES_PER_SOURCE)?;''',
            '''            // Fail startup for unsafe local explicit files, invalid directory roots,\n            // or a discovery result beyond the absolute v1 hard limit.\n            configured.backend.startup_validate(&source_id)?;''',
            "startup validate backend",
        ),
        (
            '''    #[error("configured source backend is not available yet: {source_id}/{backend}")]\n    BackendUnavailable {\n        source_id: String,\n        backend: &'static str,\n    },''',
            '''    #[error("configured source backend requires asynchronous query preparation")]\n    AsyncBackendRequired,\n\n    #[error("remote source configuration is invalid")]\n    RemoteConfigurationInvalid,\n\n    #[error("remote recursive directory discovery is not supported in the v2 MVP: {source_id}")]\n    RemoteRecursiveDiscoveryUnsupported { source_id: String },\n\n    #[error("remote configured path is invalid")]\n    RemotePathInvalid,\n\n    #[error("remote explicit file is not a regular file: {source_id}/{file_index}")]\n    RemoteExplicitFileNotRegular { source_id: String, file_index: usize },\n\n    #[error("remote snapshot is missing its cache generation pin")]\n    RemoteSnapshotMissingPin,\n\n    #[error("failed to initialize remote cache")]\n    CacheInitialization(#[source] CacheStoreError),\n\n    #[error("failed to initialize SSH transport")]\n    TransportInitialization(#[source] SshTransportError),\n\n    #[error("failed to initialize remote synchronization")]\n    SyncInitialization(#[source] SyncError),\n\n    #[error("remote source transport is unavailable: {source_id}")]\n    RemoteTransport {\n        source_id: String,\n        #[source]\n        source: SshTransportError,\n    },\n\n    #[error("remote source synchronization failed: {source_id}")]\n    RemoteSync {\n        source_id: String,\n        #[source]\n        source: SyncError,\n    },\n\n    #[error("remote refresh worker failed: {source_id}")]\n    RemoteTaskJoin {\n        source_id: String,\n        #[source]\n        source: tokio::task::JoinError,\n    },\n\n    #[error("cached generation is unavailable: {source_id}/{file_id}")]\n    CachedGenerationUnavailable {\n        source_id: String,\n        file_id: String,\n        #[source]\n        source: CacheStoreError,\n    },''',
            "remote registry errors",
        ),
    ],
)

# Queries build/refresh remote snapshots only on the first page and scan generic SnapshotFile.
for path in ["src/query_engine.rs", "src/stateful_query.rs"]:
    patch(
        path,
        [
            (
                '''    fs::File,\n    io::{ErrorKind, Read, Seek, SeekFrom},''',
                '''    io::{ErrorKind, Read, Seek, SeekFrom},''',
                f"{path} remove File import",
            ),
            (
                '''let mut file = safe_file.into_file();''',
                '''let mut file = safe_file;''',
                f"{path} generic scan reader",
            ),
            (
                '''.open_snapshot_file(&candidate.snapshot)?\n                .into_file(),''',
                '''.open_snapshot_file(&candidate.snapshot)?,''',
                f"{path} generic prefix reader",
            ),
            (
                '''fn seek_to_scan_position(\n    file: &mut File,''',
                '''fn seek_to_scan_position<R: Read + Seek>(\n    file: &mut R,''',
                f"{path} generic seek",
            ),
            (
                '''fn read_line_prefix(\n    file: &mut File,''',
                '''fn read_line_prefix<R: Read + Seek>(\n    file: &mut R,''',
                f"{path} generic prefix",
            ),
        ],
    )

patch(
    "src/query_engine.rs",
    [
        (
            '''        let candidates = build_candidates(&selected_sources, limits.max_scan_files_per_query)?;''',
            '''        let candidates =\n            build_candidates(&selected_sources, limits.max_scan_files_per_query).await?;''',
            "query engine await candidates",
        ),
        (
            '''fn build_candidates(\n    sources: &[Arc<ConfiguredSource>],\n    max_files: usize,\n) -> Result<Vec<FileCandidate>, QueryError> {''',
            '''async fn build_candidates(\n    sources: &[Arc<ConfiguredSource>],\n    max_files: usize,\n) -> Result<Vec<FileCandidate>, QueryError> {''',
            "query engine async candidates",
        ),
        (
            '''        let snapshots = source.snapshot_files(remaining)?;''',
            '''        let snapshots = source.query_snapshot_files(remaining).await?;''',
            "query engine remote snapshots",
        ),
    ],
)

patch(
    "src/stateful_query.rs",
    [
        (
            '''                build_candidates(\n                    &self.registry,\n                    &binding.source_ids,\n                    limits.max_scan_files_per_query,\n                )?,''',
            '''                build_candidates(\n                    &self.registry,\n                    &binding.source_ids,\n                    limits.max_scan_files_per_query,\n                )\n                .await?,''',
            "stateful await candidates",
        ),
        (
            '''fn build_candidates(\n    registry: &SourceRegistry,\n    source_ids: &[String],\n    max_files: usize,\n) -> Result<Vec<CursorCandidate>, StatefulQueryError> {''',
            '''async fn build_candidates(\n    registry: &SourceRegistry,\n    source_ids: &[String],\n    max_files: usize,\n) -> Result<Vec<CursorCandidate>, StatefulQueryError> {''',
            "stateful async candidates",
        ),
        (
            '''        let snapshots = source.snapshot_files(remaining)?;''',
            '''        let snapshots = source.query_snapshot_files(remaining).await?;''',
            "stateful remote snapshots",
        ),
    ],
)

# Context reader opens the exact generation encoded by a match reference identity.
patch(
    "src/context_reader.rs",
    [(
        '''    let safe_file = source.open_configured_file(&reference.relative_path)?;\n    if safe_file.identity() != reference.file_identity\n        || safe_file.size() < reference.file_size_at_match\n        || safe_file.size() < reference.match_end_offset()\n    {\n        return Err(ContextReadError::FileChanged);\n    }\n\n    let current_size = safe_file.size();\n    let mut file = safe_file.into_file();''',
        '''    let mut file = source.open_referenced_file(\n        &reference.relative_path,\n        reference.file_identity,\n        reference.file_size_at_match,\n        &reference.file_id,\n    )?;\n    if file.size() < reference.file_size_at_match || file.size() < reference.match_end_offset() {\n        return Err(ContextReadError::FileChanged);\n    }\n\n    let current_size = file.size();''',
        "context exact snapshot",
    )],
)

# Publicly expose SnapshotFile for scanner/context generic use.
patch(
    "src/lib.rs",
    [(
        '''pub use source_registry::{\n    ConfiguredSource, MAX_REGISTERED_FILES_PER_SOURCE, SourceDescriptor, SourceFileSnapshot,\n    SourceRegistry, SourceRegistryError,\n};''',
        '''pub use source_registry::{\n    ConfiguredSource, MAX_REGISTERED_FILES_PER_SOURCE, SourceDescriptor, SourceFileSnapshot,\n    SourceRegistry, SourceRegistryError,\n};\npub use backend::SnapshotFile;''',
        "export SnapshotFile",
    )],
)
