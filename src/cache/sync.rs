use std::{
    fmt,
    fmt::Write as FmtWrite,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    AppConfigV2, BackendType, BootstrapPolicy, BootstrapType, LogSourceConfigV2,
    transport::{
        MAX_READ_RANGE_BYTES, RemoteFileMetadata, RemoteFileType, SshConnectionManager,
        SshReadTransport, SshTransportError,
    },
};

use super::{
    ByteRange, CacheCoverage, CacheStore, CacheStoreError, GenerationId, GenerationMetadata,
    GenerationRecord,
};

pub const CONTINUITY_FINGERPRINT_WINDOW_BYTES: u64 = 64 * 1024;
const SYNC_READ_CHUNK_BYTES: usize = 1024 * 1024;
const FINGERPRINT_PREFIX: &str = "sha256-v1";

#[derive(Clone)]
pub struct SyncEngine {
    cache: CacheStore,
    connections: SshConnectionManager,
    max_sync_bytes_per_query: u64,
}

impl fmt::Debug for SyncEngine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SyncEngine")
            .field("cache_limits", &self.cache.limits())
            .field("connections", &self.connections)
            .field("max_sync_bytes_per_query", &self.max_sync_bytes_per_query)
            .finish()
    }
}

impl SyncEngine {
    pub fn from_config(config: &AppConfigV2, cache: CacheStore) -> Result<Self, SyncError> {
        if config.limits.max_sync_bytes_per_query == 0 {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self {
            cache,
            connections: SshConnectionManager::from_config(config)?,
            max_sync_bytes_per_query: config.limits.max_sync_bytes_per_query,
        })
    }

    pub async fn sync(&self, target: &RemoteSyncTarget) -> Result<SyncOutcome, SyncError> {
        let reader = self.connections.open_reader(target.connection_id()).await?;
        let result =
            sync_with_reader(&self.cache, target, self.max_sync_bytes_per_query, &reader).await;
        let _ = reader.close().await;
        result
    }
}

#[derive(Clone, PartialEq, Eq)]
pub struct RemoteSyncTarget {
    connection_id: String,
    source_identifier: String,
    remote_identifier: String,
    remote_path: String,
    bootstrap: BootstrapPolicy,
}

impl fmt::Debug for RemoteSyncTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteSyncTarget")
            .field("connection_id", &self.connection_id)
            .field("source_identifier", &self.source_identifier)
            .field("remote_identifier", &self.remote_identifier)
            .field("remote_path", &"<configured-remote-path>")
            .field("bootstrap", &self.bootstrap)
            .finish()
    }
}

impl RemoteSyncTarget {
    pub fn from_source(
        source: &LogSourceConfigV2,
        remote_identifier: impl Into<String>,
    ) -> Result<Self, SyncError> {
        if source.backend.backend_type != BackendType::Ssh {
            return Err(SyncError::InvalidTarget);
        }
        let connection_id = source
            .backend
            .connection_id
            .clone()
            .ok_or(SyncError::InvalidTarget)?;
        let sync = source.sync.clone().ok_or(SyncError::InvalidTarget)?;
        if sync.allow_stale_on_error {
            return Err(SyncError::InvalidTarget);
        }
        let remote_identifier = remote_identifier.into();
        let remote_path = build_remote_path(&source.root, &remote_identifier)?;

        Ok(Self {
            connection_id,
            source_identifier: source.source_id.clone(),
            remote_identifier,
            remote_path,
            bootstrap: sync.bootstrap,
        })
    }

    #[must_use]
    pub fn connection_id(&self) -> &str {
        &self.connection_id
    }

    #[must_use]
    pub fn source_identifier(&self) -> &str {
        &self.source_identifier
    }

    #[must_use]
    pub fn remote_identifier(&self) -> &str {
        &self.remote_identifier
    }
}

fn build_remote_path(root: &Path, remote_identifier: &str) -> Result<String, SyncError> {
    let root = root.to_str().ok_or(SyncError::InvalidTarget)?;
    if root.is_empty()
        || !root.starts_with('/')
        || root.len() > 4096
        || root.chars().any(char::is_control)
    {
        return Err(SyncError::InvalidTarget);
    }
    if remote_identifier.is_empty()
        || remote_identifier.len() > 4096
        || remote_identifier.starts_with('/')
        || remote_identifier.contains('\\')
        || remote_identifier.chars().any(char::is_control)
        || remote_identifier
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(SyncError::InvalidTarget);
    }

    let root = if root == "/" {
        ""
    } else {
        root.trim_end_matches('/')
    };
    Ok(format!("{root}/{remote_identifier}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncAction {
    Unchanged,
    Appended,
    NewGeneration(SyncGenerationReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncGenerationReason {
    InitialBootstrap,
    RemoteTruncated,
    MetadataChangedWithoutGrowth,
    ContinuityUnavailable,
    ContinuityMismatch,
    CacheStateMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    pub action: SyncAction,
    pub generation: GenerationId,
    pub remote_size: u64,
    pub cached_range: ByteRange,
    pub coverage: CacheCoverage,
    pub remote_bytes_read: u64,
    pub cached_bytes_written: u64,
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("remote sync configuration is invalid")]
    InvalidConfiguration,
    #[error("remote sync target is invalid")]
    InvalidTarget,
    #[error("remote log file metadata does not identify a regular file")]
    RemoteFileNotRegular,
    #[error("remote log file size is unavailable")]
    RemoteSizeUnavailable,
    #[error("remote log changed while a stable snapshot was being synchronized")]
    RemoteChangedDuringSync,
    #[error("remote sync byte limit exceeded")]
    SyncLimitExceeded,
    #[error("candidate cache generation exceeds the configured per-source capacity")]
    CacheCapacityExceeded,
    #[error(transparent)]
    Transport(#[from] SshTransportError),
    #[error(transparent)]
    Cache(#[from] CacheStoreError),
    #[error("cache I/O failed while computing a synchronization fingerprint")]
    LocalIo(#[from] std::io::Error),
}

trait RemoteSyncReader {
    async fn lstat(&self, path: &str) -> Result<RemoteFileMetadata, SyncError>;
    async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, SyncError>;
}

impl RemoteSyncReader for SshReadTransport {
    async fn lstat(&self, path: &str) -> Result<RemoteFileMetadata, SyncError> {
        Ok(SshReadTransport::lstat(self, path).await?)
    }

    async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, SyncError> {
        Ok(SshReadTransport::read_range(self, path, offset, length).await?)
    }
}

async fn sync_with_reader<R: RemoteSyncReader + Sync>(
    cache: &CacheStore,
    target: &RemoteSyncTarget,
    max_sync_bytes_per_query: u64,
    reader: &R,
) -> Result<SyncOutcome, SyncError> {
    let mut budget = SyncBudget::new(max_sync_bytes_per_query)?;
    let remote = inspect_regular_file(reader, &target.remote_path).await?;
    let manifest = cache.load_manifest(target.source_identifier(), target.remote_identifier())?;
    let Some(current) = manifest
        .as_ref()
        .and_then(|manifest| manifest.current())
        .cloned()
    else {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::InitialBootstrap,
            &mut budget,
        )
        .await;
    };

    if current.cached_range.end_exclusive != current.remote_size {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::CacheStateMismatch,
            &mut budget,
        )
        .await;
    }

    if remote.size < current.remote_size {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::RemoteTruncated,
            &mut budget,
        )
        .await;
    }

    if remote.size == current.remote_size {
        if remote.mtime_millis == current.remote_mtime_millis {
            return Ok(outcome_from_record(
                SyncAction::Unchanged,
                &current,
                budget.used(),
                0,
            ));
        }
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::MetadataChangedWithoutGrowth,
            &mut budget,
        )
        .await;
    }

    let Some(expected_fingerprint) = current
        .continuity_fingerprint
        .as_deref()
        .and_then(ContinuityFingerprint::parse)
    else {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    };
    if expected_fingerprint.end != current.remote_size {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityUnavailable,
            &mut budget,
        )
        .await;
    }

    let observed = fingerprint_remote_window(
        reader,
        &target.remote_path,
        expected_fingerprint.start,
        expected_fingerprint.end,
        &mut budget,
    )
    .await?;
    if observed != expected_fingerprint {
        return bootstrap_generation(
            cache,
            target,
            reader,
            &remote,
            SyncGenerationReason::ContinuityMismatch,
            &mut budget,
        )
        .await;
    }

    append_generation(cache, target, reader, &remote, &current, &mut budget).await
}

async fn bootstrap_generation<R: RemoteSyncReader + Sync>(
    cache: &CacheStore,
    target: &RemoteSyncTarget,
    reader: &R,
    remote: &ObservedRemoteFile,
    reason: SyncGenerationReason,
    budget: &mut SyncBudget,
) -> Result<SyncOutcome, SyncError> {
    let (cached_range, coverage) = bootstrap_range(&target.bootstrap, remote.size)?;
    ensure_candidate_fits(cache, cached_range)?;

    let mut staged =
        cache.begin_generation(target.source_identifier(), target.remote_identifier())?;
    let mut tail = TailWindow::default();
    let cached_bytes_written = copy_remote_range(
        reader,
        &target.remote_path,
        cached_range,
        &mut staged,
        &mut tail,
        budget,
    )
    .await?;

    let fingerprint = if cached_range.is_empty() {
        let start = remote
            .size
            .saturating_sub(CONTINUITY_FINGERPRINT_WINDOW_BYTES);
        fingerprint_remote_window(reader, &target.remote_path, start, remote.size, budget).await?
    } else {
        ContinuityFingerprint::from_tail(cached_range.end_exclusive, tail.as_slice())
    };

    if !cached_range.is_empty() {
        let remote_fingerprint = fingerprint_remote_window(
            reader,
            &target.remote_path,
            fingerprint.start,
            fingerprint.end,
            budget,
        )
        .await?;
        if remote_fingerprint != fingerprint {
            return Err(SyncError::RemoteChangedDuringSync);
        }
    }

    let final_metadata = inspect_regular_file(reader, &target.remote_path).await?;
    if final_metadata.size < remote.size {
        return Err(SyncError::RemoteChangedDuringSync);
    }

    let record = staged.commit(GenerationMetadata {
        remote_size: remote.size,
        cached_range,
        remote_mtime_millis: remote.mtime_millis,
        continuity_fingerprint: Some(fingerprint.encode()),
        coverage: coverage.clone(),
    })?;

    Ok(SyncOutcome {
        action: SyncAction::NewGeneration(reason),
        generation: record.generation,
        remote_size: record.remote_size,
        cached_range: record.cached_range,
        coverage,
        remote_bytes_read: budget.used(),
        cached_bytes_written,
    })
}

async fn append_generation<R: RemoteSyncReader + Sync>(
    cache: &CacheStore,
    target: &RemoteSyncTarget,
    reader: &R,
    remote: &ObservedRemoteFile,
    current: &GenerationRecord,
    budget: &mut SyncBudget,
) -> Result<SyncOutcome, SyncError> {
    let new_range =
        ByteRange::new(current.cached_range.start, remote.size).map_err(CacheStoreError::from)?;
    ensure_candidate_fits(cache, new_range)?;

    let desired_fingerprint_start = new_range.start.max(
        remote
            .size
            .saturating_sub(CONTINUITY_FINGERPRINT_WINDOW_BYTES),
    );
    let mut tail = TailWindow::default();
    let mut pinned = cache.pin_generation(
        target.source_identifier(),
        target.remote_identifier(),
        &current.generation,
    )?;
    if desired_fingerprint_start < current.remote_size {
        let local_offset = desired_fingerprint_start
            .checked_sub(current.cached_range.start)
            .ok_or(SyncError::RemoteChangedDuringSync)?;
        let local_len_u64 = current.remote_size - desired_fingerprint_start;
        let local_len =
            usize::try_from(local_len_u64).map_err(|_| SyncError::RemoteChangedDuringSync)?;
        let mut buffer = vec![0_u8; local_len];
        pinned.seek(SeekFrom::Start(local_offset))?;
        pinned.read_exact(&mut buffer)?;
        tail.push(&buffer);
    }

    let mut staged = cache.begin_append(target.source_identifier(), target.remote_identifier())?;
    if staged.generation_id() != &current.generation {
        return Err(CacheStoreError::ConcurrentGenerationChanged.into());
    }
    let append_range =
        ByteRange::new(current.remote_size, remote.size).map_err(CacheStoreError::from)?;
    let cached_bytes_written = copy_remote_range(
        reader,
        &target.remote_path,
        append_range,
        &mut staged,
        &mut tail,
        budget,
    )
    .await?;

    let fingerprint = ContinuityFingerprint::from_tail(remote.size, tail.as_slice());
    if fingerprint.start != desired_fingerprint_start {
        return Err(SyncError::RemoteChangedDuringSync);
    }
    let remote_fingerprint = fingerprint_remote_window(
        reader,
        &target.remote_path,
        fingerprint.start,
        fingerprint.end,
        budget,
    )
    .await?;
    if remote_fingerprint != fingerprint {
        return Err(SyncError::RemoteChangedDuringSync);
    }

    let final_metadata = inspect_regular_file(reader, &target.remote_path).await?;
    if final_metadata.size < remote.size {
        return Err(SyncError::RemoteChangedDuringSync);
    }

    let record = staged.commit(GenerationMetadata {
        remote_size: remote.size,
        cached_range: new_range,
        remote_mtime_millis: remote.mtime_millis,
        continuity_fingerprint: Some(fingerprint.encode()),
        coverage: current.coverage.clone(),
    })?;

    Ok(SyncOutcome {
        action: SyncAction::Appended,
        generation: record.generation,
        remote_size: record.remote_size,
        cached_range: record.cached_range,
        coverage: record.coverage,
        remote_bytes_read: budget.used(),
        cached_bytes_written,
    })
}

fn bootstrap_range(
    bootstrap: &BootstrapPolicy,
    remote_size: u64,
) -> Result<(ByteRange, CacheCoverage), SyncError> {
    match bootstrap.bootstrap_type {
        BootstrapType::Full => Ok((
            ByteRange::new(0, remote_size).map_err(CacheStoreError::from)?,
            CacheCoverage::Full,
        )),
        BootstrapType::Tail => {
            let bytes = bootstrap.bytes.ok_or(SyncError::InvalidTarget)?;
            let start = remote_size.saturating_sub(bytes);
            Ok((
                ByteRange::new(start, remote_size).map_err(CacheStoreError::from)?,
                CacheCoverage::Tail {
                    start_offset: start,
                },
            ))
        }
        BootstrapType::FromNow => Ok((
            ByteRange::new(remote_size, remote_size).map_err(CacheStoreError::from)?,
            CacheCoverage::FromNow {
                start_offset: remote_size,
            },
        )),
    }
}

async fn copy_remote_range<R: RemoteSyncReader + Sync, W: Write>(
    reader: &R,
    path: &str,
    range: ByteRange,
    writer: &mut W,
    tail: &mut TailWindow,
    budget: &mut SyncBudget,
) -> Result<u64, SyncError> {
    let mut offset = range.start;
    while offset < range.end_exclusive {
        let remaining = range.end_exclusive - offset;
        let chunk = remaining
            .min(u64::try_from(SYNC_READ_CHUNK_BYTES).unwrap_or(u64::MAX))
            .min(u64::try_from(MAX_READ_RANGE_BYTES).unwrap_or(u64::MAX));
        let length = usize::try_from(chunk).map_err(|_| SyncError::SyncLimitExceeded)?;
        budget.consume(chunk)?;
        let bytes = reader.read_range(path, offset, length).await?;
        if bytes.len() != length {
            return Err(SyncError::RemoteChangedDuringSync);
        }
        writer.write_all(&bytes)?;
        tail.push(&bytes);
        offset = offset
            .checked_add(chunk)
            .ok_or(SyncError::RemoteChangedDuringSync)?;
    }
    Ok(range.len())
}

async fn fingerprint_remote_window<R: RemoteSyncReader + Sync>(
    reader: &R,
    path: &str,
    start: u64,
    end: u64,
    budget: &mut SyncBudget,
) -> Result<ContinuityFingerprint, SyncError> {
    if end < start || end - start > CONTINUITY_FINGERPRINT_WINDOW_BYTES {
        return Err(SyncError::RemoteChangedDuringSync);
    }
    let length_u64 = end - start;
    if length_u64 == 0 {
        return Ok(ContinuityFingerprint::from_bytes(start, end, &[]));
    }
    let length = usize::try_from(length_u64).map_err(|_| SyncError::SyncLimitExceeded)?;
    budget.consume(length_u64)?;
    let bytes = reader.read_range(path, start, length).await?;
    if bytes.len() != length {
        return Err(SyncError::RemoteChangedDuringSync);
    }
    Ok(ContinuityFingerprint::from_bytes(start, end, &bytes))
}

async fn inspect_regular_file<R: RemoteSyncReader + Sync>(
    reader: &R,
    path: &str,
) -> Result<ObservedRemoteFile, SyncError> {
    let metadata = reader.lstat(path).await?;
    if metadata.file_type != RemoteFileType::Regular {
        return Err(SyncError::RemoteFileNotRegular);
    }
    let size = metadata.size.ok_or(SyncError::RemoteSizeUnavailable)?;
    Ok(ObservedRemoteFile {
        size,
        mtime_millis: metadata.mtime.map(|value| i64::from(value) * 1000),
    })
}

fn ensure_candidate_fits(cache: &CacheStore, range: ByteRange) -> Result<(), SyncError> {
    let len = range.len();
    let limits = cache.limits();
    if len > limits.max_bytes_per_source || len > limits.max_bytes {
        return Err(SyncError::CacheCapacityExceeded);
    }
    Ok(())
}

fn outcome_from_record(
    action: SyncAction,
    record: &GenerationRecord,
    remote_bytes_read: u64,
    cached_bytes_written: u64,
) -> SyncOutcome {
    SyncOutcome {
        action,
        generation: record.generation.clone(),
        remote_size: record.remote_size,
        cached_range: record.cached_range,
        coverage: record.coverage.clone(),
        remote_bytes_read,
        cached_bytes_written,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ObservedRemoteFile {
    size: u64,
    mtime_millis: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContinuityFingerprint {
    start: u64,
    end: u64,
    digest_hex: String,
}

impl ContinuityFingerprint {
    fn from_tail(end: u64, bytes: &[u8]) -> Self {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        Self::from_bytes(end.saturating_sub(len), end, bytes)
    }

    fn from_bytes(start: u64, end: u64, bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        let mut digest_hex = String::with_capacity(digest.len() * 2);
        for byte in digest {
            let _ = write!(&mut digest_hex, "{byte:02x}");
        }
        Self {
            start,
            end,
            digest_hex,
        }
    }

    fn encode(&self) -> String {
        format!(
            "{FINGERPRINT_PREFIX}:{}:{}:{}",
            self.start, self.end, self.digest_hex
        )
    }

    fn parse(value: &str) -> Option<Self> {
        let mut parts = value.split(':');
        if parts.next()? != FINGERPRINT_PREFIX {
            return None;
        }
        let start = parts.next()?.parse().ok()?;
        let end = parts.next()?.parse().ok()?;
        let digest_hex = parts.next()?.to_owned();
        if parts.next().is_some()
            || end < start
            || end - start > CONTINUITY_FINGERPRINT_WINDOW_BYTES
            || digest_hex.len() != 64
            || !digest_hex.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return None;
        }
        Some(Self {
            start,
            end,
            digest_hex,
        })
    }
}

#[derive(Default)]
struct TailWindow {
    bytes: Vec<u8>,
}

impl TailWindow {
    fn push(&mut self, bytes: &[u8]) {
        let window = usize::try_from(CONTINUITY_FINGERPRINT_WINDOW_BYTES).unwrap_or(usize::MAX);
        if bytes.len() >= window {
            self.bytes.clear();
            self.bytes.extend_from_slice(&bytes[bytes.len() - window..]);
            return;
        }
        let total = self.bytes.len().saturating_add(bytes.len());
        if total > window {
            let remove = total - window;
            self.bytes.drain(..remove);
        }
        self.bytes.extend_from_slice(bytes);
    }

    fn as_slice(&self) -> &[u8] {
        &self.bytes
    }
}

struct SyncBudget {
    max: u64,
    used: u64,
}

impl SyncBudget {
    fn new(max: u64) -> Result<Self, SyncError> {
        if max == 0 {
            return Err(SyncError::InvalidConfiguration);
        }
        Ok(Self { max, used: 0 })
    }

    fn consume(&mut self, bytes: u64) -> Result<(), SyncError> {
        let next = self
            .used
            .checked_add(bytes)
            .ok_or(SyncError::SyncLimitExceeded)?;
        if next > self.max {
            return Err(SyncError::SyncLimitExceeded);
        }
        self.used = next;
        Ok(())
    }

    fn used(&self) -> u64 {
        self.used
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::CacheStoreLimits;

    struct FakeReader {
        bytes: Vec<u8>,
        mtime: Option<u32>,
        fail_at_or_after: Option<u64>,
        reads: Mutex<Vec<(u64, usize)>>,
    }

    impl FakeReader {
        fn new(bytes: &[u8], mtime: u32) -> Self {
            Self {
                bytes: bytes.to_vec(),
                mtime: Some(mtime),
                fail_at_or_after: None,
                reads: Mutex::new(Vec::new()),
            }
        }

        fn failing(bytes: &[u8], mtime: u32, fail_at_or_after: u64) -> Self {
            Self {
                bytes: bytes.to_vec(),
                mtime: Some(mtime),
                fail_at_or_after: Some(fail_at_or_after),
                reads: Mutex::new(Vec::new()),
            }
        }

        fn reads(&self) -> Vec<(u64, usize)> {
            self.reads.lock().expect("reads lock").clone()
        }
    }

    impl RemoteSyncReader for FakeReader {
        async fn lstat(&self, _path: &str) -> Result<RemoteFileMetadata, SyncError> {
            Ok(RemoteFileMetadata {
                size: Some(u64::try_from(self.bytes.len()).expect("size")),
                permissions: Some(0o100444),
                mtime: self.mtime,
                file_type: RemoteFileType::Regular,
            })
        }

        async fn read_range(
            &self,
            _path: &str,
            offset: u64,
            length: usize,
        ) -> Result<Vec<u8>, SyncError> {
            self.reads
                .lock()
                .expect("reads lock")
                .push((offset, length));
            if self
                .fail_at_or_after
                .is_some_and(|fail_at| offset >= fail_at)
            {
                return Err(SshTransportError::SftpProtocol.into());
            }
            let start = usize::try_from(offset).map_err(|_| SyncError::RemoteChangedDuringSync)?;
            let end = start
                .checked_add(length)
                .ok_or(SyncError::RemoteChangedDuringSync)?;
            if end > self.bytes.len() {
                return Err(SyncError::RemoteChangedDuringSync);
            }
            Ok(self.bytes[start..end].to_vec())
        }
    }

    fn cache(temp: &TempDir) -> CacheStore {
        CacheStore::open(
            temp.path(),
            CacheStoreLimits {
                max_bytes: 1024 * 1024,
                max_bytes_per_source: 1024 * 1024,
                retention: std::time::Duration::from_secs(3600),
                max_generations_per_file: 4,
            },
        )
        .expect("cache")
    }

    fn target(bootstrap_type: BootstrapType, bytes: Option<u64>) -> RemoteSyncTarget {
        RemoteSyncTarget {
            connection_id: "test-connection".to_owned(),
            source_identifier: "service-a".to_owned(),
            remote_identifier: "logs/application.log".to_owned(),
            remote_path: "/var/log/application.log".to_owned(),
            bootstrap: BootstrapPolicy {
                bootstrap_type,
                bytes,
            },
        }
    }

    async fn run(
        cache: &CacheStore,
        target: &RemoteSyncTarget,
        reader: &FakeReader,
        limit: u64,
    ) -> Result<SyncOutcome, SyncError> {
        sync_with_reader(cache, target, limit, reader).await
    }

    #[test]
    fn configured_target_derives_remote_path_and_rejects_escape() {
        const CONFIG: &str = include_str!("../../tests/contracts/v2/valid/ssh-password-tail.json");
        let config = AppConfigV2::from_json_str(CONFIG).expect("config");
        let source = &config.sources[0];
        let target = RemoteSyncTarget::from_source(source, "application.log").expect("target");
        assert_eq!(
            target.remote_path,
            "/data/log/order-service/application.log"
        );
        assert!(matches!(
            RemoteSyncTarget::from_source(source, "../secret.log"),
            Err(SyncError::InvalidTarget)
        ));
        assert!(matches!(
            RemoteSyncTarget::from_source(source, "/etc/passwd"),
            Err(SyncError::InvalidTarget)
        ));
    }

    #[tokio::test]
    async fn full_bootstrap_publishes_stable_generation() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let reader = FakeReader::new(b"abcdef", 1);

        let outcome = run(&cache, &target, &reader, 1024).await.expect("sync");
        assert_eq!(
            outcome.action,
            SyncAction::NewGeneration(SyncGenerationReason::InitialBootstrap)
        );
        assert_eq!(outcome.cached_range, ByteRange::new(0, 6).expect("range"));
        assert_eq!(outcome.coverage, CacheCoverage::Full);
        assert_eq!(outcome.cached_bytes_written, 6);

        let mut pinned = cache
            .pin_current_generation("service-a", "logs/application.log")
            .expect("pin");
        let mut text = String::new();
        pinned.read_to_string(&mut text).expect("read");
        assert_eq!(text, "abcdef");
    }

    #[tokio::test]
    async fn tail_bootstrap_only_caches_requested_tail() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Tail, Some(3));
        let reader = FakeReader::new(b"abcdef", 1);

        let outcome = run(&cache, &target, &reader, 1024).await.expect("sync");
        assert_eq!(outcome.cached_range, ByteRange::new(3, 6).expect("range"));
        assert_eq!(outcome.coverage, CacheCoverage::Tail { start_offset: 3 });
        let mut pinned = cache
            .pin_current_generation("service-a", "logs/application.log")
            .expect("pin");
        let mut text = String::new();
        pinned.read_to_string(&mut text).expect("read");
        assert_eq!(text, "def");
    }

    #[tokio::test]
    async fn from_now_bootstrap_then_append_preserves_generation() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::FromNow, None);
        let first_reader = FakeReader::new(b"before", 1);
        let first = run(&cache, &target, &first_reader, 1024)
            .await
            .expect("bootstrap");
        assert_eq!(first.cached_range, ByteRange::new(6, 6).expect("range"));

        let second_reader = FakeReader::new(b"before-after", 2);
        let second = run(&cache, &target, &second_reader, 1024)
            .await
            .expect("append");
        assert_eq!(second.action, SyncAction::Appended);
        assert_eq!(second.generation, first.generation);
        assert_eq!(second.cached_range, ByteRange::new(6, 12).expect("range"));

        let mut pinned = cache
            .pin_current_generation("service-a", "logs/application.log")
            .expect("pin");
        let mut text = String::new();
        pinned.read_to_string(&mut text).expect("read");
        assert_eq!(text, "-after");
    }

    #[tokio::test]
    async fn unchanged_metadata_does_not_download_ranges() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");

        let reader = FakeReader::new(b"abcdef", 1);
        let outcome = run(&cache, &target, &reader, 1024).await.expect("sync");
        assert_eq!(outcome.action, SyncAction::Unchanged);
        assert!(reader.reads().is_empty());
    }

    #[tokio::test]
    async fn append_downloads_new_range_after_continuity_probe() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");

        let reader = FakeReader::new(b"abcdefXYZ", 2);
        let outcome = run(&cache, &target, &reader, 1024).await.expect("append");
        assert_eq!(outcome.action, SyncAction::Appended);
        assert_eq!(outcome.generation, first.generation);
        assert_eq!(outcome.cached_bytes_written, 3);
        assert!(
            reader
                .reads()
                .iter()
                .any(|(offset, length)| *offset == 6 && *length == 3)
        );
    }

    #[tokio::test]
    async fn truncate_creates_new_generation_and_retains_old() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");

        let second = run(&cache, &target, &FakeReader::new(b"xy", 2), 1024)
            .await
            .expect("truncate sync");
        assert_eq!(
            second.action,
            SyncAction::NewGeneration(SyncGenerationReason::RemoteTruncated)
        );
        assert_ne!(second.generation, first.generation);
        let manifest = cache
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
            manifest
                .generations
                .iter()
                .any(|record| record.generation == second.generation)
        );
    }

    #[tokio::test]
    async fn same_size_mtime_change_is_treated_as_replacement() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 1024)
            .await
            .expect("bootstrap");
        let second = run(&cache, &target, &FakeReader::new(b"uvwxyz", 2), 1024)
            .await
            .expect("replacement");
        assert_eq!(
            second.action,
            SyncAction::NewGeneration(SyncGenerationReason::MetadataChangedWithoutGrowth)
        );
        assert_ne!(second.generation, first.generation);
    }

    #[tokio::test]
    async fn rapid_truncate_and_growth_is_caught_by_continuity_mismatch() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 2048)
            .await
            .expect("bootstrap");
        let second = run(&cache, &target, &FakeReader::new(b"123456789", 2), 2048)
            .await
            .expect("replacement");
        assert_eq!(
            second.action,
            SyncAction::NewGeneration(SyncGenerationReason::ContinuityMismatch)
        );
        assert_ne!(second.generation, first.generation);
    }

    #[tokio::test]
    async fn interrupted_append_preserves_last_valid_generation() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let first = run(&cache, &target, &FakeReader::new(b"abcdef", 1), 2048)
            .await
            .expect("bootstrap");

        let reader = FakeReader::failing(b"abcdefXYZ", 2, 6);
        assert!(run(&cache, &target, &reader, 2048).await.is_err());
        let manifest = cache
            .load_manifest("service-a", "logs/application.log")
            .expect("manifest")
            .expect("present");
        assert_eq!(manifest.current_generation, Some(first.generation));
        assert_eq!(manifest.current().expect("current").remote_size, 6);
    }

    #[tokio::test]
    async fn bootstrap_limit_failure_does_not_publish_manifest() {
        let temp = TempDir::new().expect("temp");
        let cache = cache(&temp);
        let target = target(BootstrapType::Full, None);
        let reader = FakeReader::new(b"abcdef", 1);
        assert!(matches!(
            run(&cache, &target, &reader, 5).await,
            Err(SyncError::SyncLimitExceeded)
        ));
        assert!(
            cache
                .load_manifest("service-a", "logs/application.log")
                .expect("manifest")
                .is_none()
        );
    }
}
