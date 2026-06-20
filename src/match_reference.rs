use std::{
    collections::{HashMap, VecDeque},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    FileIdentity, MAX_KEYWORD_CHARS, MAX_SOURCE_ID_CHARS, SafeFile, SafeOpenError, SafeRoot,
    ScanMatch,
};

const MATCH_REFERENCE_PREFIX: &str = "mref_";
const MATCH_REFERENCE_LENGTH: usize = MATCH_REFERENCE_PREFIX.len() + 32;

/// Server-internal metadata associated with an opaque `match_ref`.
///
/// This type intentionally does not implement `Serialize`; its relative path,
/// inode and offsets must never be returned through MCP responses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReferenceData {
    pub source_id: String,
    pub relative_path: PathBuf,
    pub file_identity: FileIdentity,
    pub file_size_at_match: u64,
    pub line_number: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
    pub keyword: String,
    pub case_sensitive: bool,
}

impl MatchReferenceData {
    pub fn from_scan_match(
        source_id: impl Into<String>,
        relative_path: impl Into<PathBuf>,
        file_identity: FileIdentity,
        file_size_at_match: u64,
        keyword: impl Into<String>,
        case_sensitive: bool,
        scan_match: &ScanMatch,
    ) -> Result<Self, MatchReferenceError> {
        let data = Self {
            source_id: source_id.into(),
            relative_path: relative_path.into(),
            file_identity,
            file_size_at_match,
            line_number: scan_match.line_number,
            line_start_offset: scan_match.line_start_offset,
            match_byte_offset: scan_match.match_byte_offset,
            keyword: keyword.into(),
            case_sensitive,
        };
        data.validate()?;
        Ok(data)
    }

    pub fn validate(&self) -> Result<(), MatchReferenceError> {
        let source_chars = self.source_id.chars().count();
        if source_chars == 0 || source_chars > MAX_SOURCE_ID_CHARS {
            return Err(MatchReferenceError::InvalidData(
                "source_id length is outside the server limit",
            ));
        }
        validate_relative_path(&self.relative_path)?;

        let keyword_bytes = self.keyword.as_bytes();
        if keyword_bytes.is_empty()
            || self.keyword.chars().count() > MAX_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(MatchReferenceError::InvalidData(
                "keyword is not a valid literal log search term",
            ));
        }
        if self.line_number == 0 {
            return Err(MatchReferenceError::InvalidData(
                "line_number must start at one",
            ));
        }
        if self.line_start_offset > self.match_byte_offset {
            return Err(MatchReferenceError::InvalidData(
                "match offset precedes its line start",
            ));
        }

        let keyword_len = u64::try_from(keyword_bytes.len()).map_err(|_| {
            MatchReferenceError::InvalidData("keyword byte length cannot be represented")
        })?;
        let match_end = self.match_byte_offset.checked_add(keyword_len).ok_or(
            MatchReferenceError::InvalidData("match byte range overflows"),
        )?;
        if match_end > self.file_size_at_match {
            return Err(MatchReferenceError::InvalidData(
                "match byte range exceeds the scanned file",
            ));
        }

        Ok(())
    }

    #[must_use]
    pub fn match_end_offset(&self) -> u64 {
        self.match_byte_offset
            .saturating_add(u64::try_from(self.keyword.len()).unwrap_or(u64::MAX))
    }
}

#[derive(Debug)]
pub struct MatchReferenceStore {
    capacity: usize,
    ttl: Duration,
    state: Mutex<StoreState>,
}

impl MatchReferenceStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, MatchReferenceError> {
        if capacity == 0 {
            return Err(MatchReferenceError::InvalidCapacity);
        }
        if ttl == Duration::ZERO {
            return Err(MatchReferenceError::InvalidTtl);
        }

        Ok(Self {
            capacity,
            ttl,
            state: Mutex::new(StoreState::default()),
        })
    }

    pub fn insert(&self, data: MatchReferenceData) -> Result<String, MatchReferenceError> {
        data.validate()?;
        let now = Instant::now();
        let expires_at = now
            .checked_add(self.ttl)
            .ok_or(MatchReferenceError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);

        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }

        let token = loop {
            let candidate = format!("{MATCH_REFERENCE_PREFIX}{}", Uuid::new_v4().simple());
            if !state.entries.contains_key(&candidate) {
                break candidate;
            }
        };

        state.order.push_back(token.clone());
        state
            .entries
            .insert(token.clone(), StoredReference { data, expires_at });

        Ok(token)
    }

    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, MatchReferenceError> {
        if !is_well_formed_token(token) {
            return Err(MatchReferenceError::UnknownOrExpired);
        }

        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state
            .entries
            .get(token)
            .map(|entry| entry.data.clone())
            .ok_or(MatchReferenceError::UnknownOrExpired)
    }

    pub fn remove(&self, token: &str) -> bool {
        self.lock_state().entries.remove(token).is_some()
    }

    pub fn purge_expired(&self) -> usize {
        self.lock_state().purge_expired(Instant::now())
    }

    pub fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock_state(&self) -> MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Default)]
struct StoreState {
    entries: HashMap<String, StoredReference>,
    order: VecDeque<String>,
}

impl StoreState {
    fn purge_expired(&mut self, now: Instant) -> usize {
        let before = self.entries.len();

        while let Some(token) = self.order.front().cloned() {
            match self.entries.get(&token) {
                Some(entry) if entry.expires_at <= now => {
                    self.order.pop_front();
                    self.entries.remove(&token);
                }
                None => {
                    self.order.pop_front();
                }
                Some(_) => break,
            }
        }

        before - self.entries.len()
    }

    fn evict_oldest(&mut self) {
        while let Some(token) = self.order.pop_front() {
            if self.entries.remove(&token).is_some() {
                return;
            }
        }
    }
}

#[derive(Debug)]
struct StoredReference {
    data: MatchReferenceData,
    expires_at: Instant,
}

fn is_well_formed_token(token: &str) -> bool {
    token.len() == MATCH_REFERENCE_LENGTH
        && token.starts_with(MATCH_REFERENCE_PREFIX)
        && token[MATCH_REFERENCE_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn validate_relative_path(path: &Path) -> Result<(), MatchReferenceError> {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(MatchReferenceError::InvalidData(
                    "relative_path must be normalized and remain below its source root",
                ));
            }
        }
    }

    if !has_component {
        return Err(MatchReferenceError::InvalidData(
            "relative_path must contain a file name",
        ));
    }
    Ok(())
}

pub fn open_referenced_file(
    root: &SafeRoot,
    reference: &MatchReferenceData,
) -> Result<SafeFile, MatchReferenceFileError> {
    reference.validate()?;
    let safe_file = root.open_regular_file(&reference.relative_path)?;
    if safe_file.identity() != reference.file_identity
        || safe_file.size() < reference.match_end_offset()
    {
        return Err(MatchReferenceFileError::FileChanged);
    }

    Ok(safe_file)
}

#[derive(Debug, Error)]
pub enum MatchReferenceError {
    #[error("match reference capacity must be greater than zero")]
    InvalidCapacity,

    #[error("match reference TTL must be greater than zero")]
    InvalidTtl,

    #[error("match reference expiration cannot be represented")]
    ExpirationOverflow,

    #[error("invalid match reference data: {0}")]
    InvalidData(&'static str),

    #[error("unknown or expired match reference")]
    UnknownOrExpired,
}

#[derive(Debug, Error)]
pub enum MatchReferenceFileError {
    #[error("match reference metadata is invalid")]
    InvalidReference(#[from] MatchReferenceError),

    #[error("referenced log file cannot be opened safely")]
    Open(#[from] SafeOpenError),

    #[error("referenced log file has been rotated, replaced or truncated")]
    FileChanged,
}

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use tempfile::tempdir;

    use super::*;

    fn sample_match() -> ScanMatch {
        ScanMatch {
            line_number: 2,
            line_start_offset: 20,
            match_byte_offset: 26,
            content: "ERROR abc123".to_owned(),
            content_truncated: false,
            content_lossy: false,
            original_line_bytes: 12,
        }
    }

    fn sample_data() -> MatchReferenceData {
        MatchReferenceData::from_scan_match(
            "payment-test",
            "application.log",
            FileIdentity {
                device: 10,
                inode: 20,
            },
            64,
            "abc123",
            false,
            &sample_match(),
        )
        .expect("sample reference should be valid")
    }

    #[test]
    fn creates_opaque_unique_references_and_resolves_data() {
        let store =
            MatchReferenceStore::new(10, Duration::from_secs(60)).expect("store should be created");
        let first = store
            .insert(sample_data())
            .expect("reference should be inserted");
        let second = store
            .insert(sample_data())
            .expect("second reference should be inserted");

        assert_ne!(first, second);
        assert!(first.starts_with(MATCH_REFERENCE_PREFIX));
        assert_eq!(first.len(), MATCH_REFERENCE_LENGTH);
        assert!(!first.contains("payment"));
        assert!(!first.contains("application"));
        assert_eq!(
            store.resolve(&first).expect("reference should resolve"),
            sample_data()
        );
    }

    #[test]
    fn rejects_unknown_and_modified_references() {
        let store =
            MatchReferenceStore::new(10, Duration::from_secs(60)).expect("store should be created");
        let token = store
            .insert(sample_data())
            .expect("reference should be inserted");
        let mut modified = token.clone();
        modified.replace_range(modified.len() - 1.., "z");

        assert!(matches!(
            store.resolve("mref_not-a-valid-token"),
            Err(MatchReferenceError::UnknownOrExpired)
        ));
        assert!(matches!(
            store.resolve(&modified),
            Err(MatchReferenceError::UnknownOrExpired)
        ));
    }

    #[test]
    fn expires_references_and_purges_them() {
        let store = MatchReferenceStore::new(10, Duration::from_millis(5))
            .expect("store should be created");
        let token = store
            .insert(sample_data())
            .expect("reference should be inserted");

        thread::sleep(Duration::from_millis(20));

        assert!(matches!(
            store.resolve(&token),
            Err(MatchReferenceError::UnknownOrExpired)
        ));
        assert!(store.is_empty());
    }

    #[test]
    fn evicts_oldest_reference_at_capacity() {
        let store =
            MatchReferenceStore::new(2, Duration::from_secs(60)).expect("store should be created");
        let first = store
            .insert(sample_data())
            .expect("first reference should be inserted");
        let second = store
            .insert(sample_data())
            .expect("second reference should be inserted");
        let third = store
            .insert(sample_data())
            .expect("third reference should be inserted");

        assert!(matches!(
            store.resolve(&first),
            Err(MatchReferenceError::UnknownOrExpired)
        ));
        assert!(store.resolve(&second).is_ok());
        assert!(store.resolve(&third).is_ok());
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn restart_or_new_store_invalidates_existing_token() {
        let first_store =
            MatchReferenceStore::new(10, Duration::from_secs(60)).expect("store should be created");
        let token = first_store
            .insert(sample_data())
            .expect("reference should be inserted");
        let new_store = MatchReferenceStore::new(10, Duration::from_secs(60))
            .expect("new store should be created");

        assert!(matches!(
            new_store.resolve(&token),
            Err(MatchReferenceError::UnknownOrExpired)
        ));
    }

    #[test]
    fn rejects_untrusted_paths_and_inconsistent_offsets() {
        let mut data = sample_data();
        data.relative_path = PathBuf::from("../outside.log");
        assert!(matches!(
            data.validate(),
            Err(MatchReferenceError::InvalidData(_))
        ));

        let mut data = sample_data();
        data.match_byte_offset = 10;
        assert!(matches!(
            data.validate(),
            Err(MatchReferenceError::InvalidData(_))
        ));

        let mut data = sample_data();
        data.file_size_at_match = 28;
        assert!(matches!(
            data.validate(),
            Err(MatchReferenceError::InvalidData(_))
        ));
    }

    #[test]
    fn verifies_file_identity_before_context_reading() {
        let root_dir = tempdir().expect("temporary root should be created");
        let log_path = root_dir.path().join("application.log");
        let content = "prefix traceId=abc123\n";
        fs::write(&log_path, content).expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let scan_match = ScanMatch {
            line_number: 1,
            line_start_offset: 0,
            match_byte_offset: 15,
            content: content.trim_end().to_owned(),
            content_truncated: false,
            content_lossy: false,
            original_line_bytes: 21,
        };
        let reference = MatchReferenceData::from_scan_match(
            "payment-test",
            "application.log",
            safe_file.identity(),
            safe_file.size(),
            "abc123",
            false,
            &scan_match,
        )
        .expect("reference should be valid");

        let reopened = open_referenced_file(&root, &reference)
            .expect("unchanged referenced file should reopen");
        assert_eq!(reopened.identity(), reference.file_identity);
    }

    #[test]
    fn rejects_file_replaced_after_reference_creation() {
        let root_dir = tempdir().expect("temporary root should be created");
        let log_path = root_dir.path().join("application.log");
        let rotated_path = root_dir.path().join("application.log.1");
        fs::write(&log_path, "original abc123\n").expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let scan_match = ScanMatch {
            line_number: 1,
            line_start_offset: 0,
            match_byte_offset: 9,
            content: "original abc123".to_owned(),
            content_truncated: false,
            content_lossy: false,
            original_line_bytes: 15,
        };
        let reference = MatchReferenceData::from_scan_match(
            "payment-test",
            "application.log",
            safe_file.identity(),
            safe_file.size(),
            "abc123",
            false,
            &scan_match,
        )
        .expect("reference should be valid");

        fs::rename(&log_path, &rotated_path).expect("original file should rotate");
        fs::write(&log_path, "replacement abc123\n").expect("replacement should be written");

        assert!(matches!(
            open_referenced_file(&root, &reference),
            Err(MatchReferenceFileError::FileChanged)
        ));
    }

    #[test]
    fn validates_store_configuration() {
        assert!(matches!(
            MatchReferenceStore::new(0, Duration::from_secs(1)),
            Err(MatchReferenceError::InvalidCapacity)
        ));
        assert!(matches!(
            MatchReferenceStore::new(1, Duration::ZERO),
            Err(MatchReferenceError::InvalidTtl)
        ));
    }
}
