from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exact pattern once, got {count}")
    return text.replace(old, new)


path = Path("src/cache/store.rs")
text = path.read_text()

anchor = '''        Ok(StagedGeneration {
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

    pub fn load_manifest('''
replacement = '''        Ok(StagedGeneration {
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

    pub fn load_manifest('''
text = replace_once(text, anchor, replacement, "begin_append insertion")

old = '''        Ok(PinnedGeneration {
            store: self.clone(),
            key,
            file,
            record,
        })'''
new = '''        let limit = record.data_len;
        Ok(PinnedGeneration {
            store: self.clone(),
            key,
            file,
            record,
            position: 0,
            limit,
        })'''
text = replace_once(text, old, new, "pinned constructor")

old = '''                    let file = open_regular_private_file(&path)?;
                    let actual_len = file.metadata()?.len();
                    if actual_len != generation.data_len {
                        return Err(CacheStoreError::GenerationLengthMismatch {
                            expected: generation.data_len,
                            actual: actual_len,
                        });
                    }
                    referenced.insert(path, ());'''
new = '''                    let file = open_regular_private_file_for_update(&path)?;
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
                    referenced.insert(path, ());'''
text = replace_once(text, old, new, "recovery append repair")

old = '''impl Drop for StagedGeneration {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = fs::remove_file(&self.staging_path);
        }
    }
}

pub struct PinnedGeneration {'''
new = '''impl Drop for StagedGeneration {
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

pub struct PinnedGeneration {'''
text = replace_once(text, old, new, "StagedAppend insertion")

old = '''pub struct PinnedGeneration {
    store: CacheStore,
    key: GenerationKey,
    file: File,
    record: GenerationRecord,
}'''
new = '''pub struct PinnedGeneration {
    store: CacheStore,
    key: GenerationKey,
    file: File,
    record: GenerationRecord,
    position: u64,
    limit: u64,
}'''
text = replace_once(text, old, new, "PinnedGeneration fields")

old = '''impl Read for PinnedGeneration {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.file.read(buffer)
    }
}

impl Seek for PinnedGeneration {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        self.file.seek(position)
    }
}'''
new = '''impl Read for PinnedGeneration {
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
}'''
text = replace_once(text, old, new, "bounded pinned IO")

old = '''pub struct RecoveryReport {
    pub manifests: usize,
    pub generations: usize,
    pub orphan_staging_removed: usize,
    pub orphan_generations_removed: usize,
}'''
new = '''pub struct RecoveryReport {
    pub manifests: usize,
    pub generations: usize,
    pub orphan_staging_removed: usize,
    pub orphan_generations_removed: usize,
    pub repaired_appends: usize,
}'''
text = replace_once(text, old, new, "RecoveryReport")

old = '''    #[error("cache staging file is already closed")]
    StagingClosed,
    #[error("cache metadata lock is poisoned")]'''
new = '''    #[error("cache staging file is already closed")]
    StagingClosed,
    #[error("append metadata does not extend the current cached range exactly")]
    AppendRangeMismatch,
    #[error("cache generation changed while append data was staged")]
    ConcurrentGenerationChanged,
    #[error("cache metadata lock is poisoned")]'''
text = replace_once(text, old, new, "append errors")

old = '''fn open_regular_private_file(path: &Path) -> Result<File, CacheStoreError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CacheStoreError::InvalidLayout);
    }
    set_private_file(path)?;
    Ok(File::open(path)?)
}

fn set_private_file'''
new = '''fn open_regular_private_file(path: &Path) -> Result<File, CacheStoreError> {
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

fn set_private_file'''
text = replace_once(text, old, new, "update file helper")

old = '''    fn pinned_old_generation_is_not_removed_by_gc() {'''
tests = '''    fn append_keeps_generation_and_pinned_snapshot_length() {
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
        let mut data = OpenOptions::new().append(true).open(&path).expect("open data");
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
    fn pinned_old_generation_is_not_removed_by_gc() {'''
text = replace_once(text, old, tests, "append tests")
path.write_text(text)

mod_path = Path("src/cache/mod.rs")
mod_text = mod_path.read_text()
mod_text = replace_once(
    mod_text,
    '    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,\n    StagedGeneration,\n',
    '    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,\n    StagedAppend, StagedGeneration,\n',
    "cache module export",
)
mod_path.write_text(mod_text)

lib_path = Path("src/lib.rs")
lib_text = lib_path.read_text()
lib_text = replace_once(
    lib_text,
    '    PinnedGeneration, RecoveryReport, StagedGeneration,\n',
    '    PinnedGeneration, RecoveryReport, StagedAppend, StagedGeneration,\n',
    "crate export",
)
lib_path.write_text(lib_text)
