use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{FileIdentity, SafeFile, SafeOpenError, SafeRoot};

const MATCH_REFERENCE_PREFIX: &str = "mref_";
const MATCH_REFERENCE_LENGTH: usize = MATCH_REFERENCE_PREFIX.len() + 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReferenceData {
    pub source_id: String,
    pub relative_path: PathBuf,
    pub file_identity: FileIdentity,
    pub line_number: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
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

        loop {
            let Some(token) = self.order.front() else {
                break;
            };

            match self.entries.get(token) {
                Some(entry) if entry.expires_at <= now => {
                    let token = self.order.pop_front().expect("front token exists");
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

pub fn open_referenced_file(
    root: &SafeRoot,
    reference: &MatchReferenceData,
) -> Result<SafeFile, MatchReferenceFileError> {
    let safe_file = root.open_regular_file(&reference.relative_path)?;
    if safe_file.identity() != reference.file_identity {
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

    #[error("unknown or expired match reference")]
    UnknownOrExpired,
}

#[derive(Debug, Error)]
pub enum MatchReferenceFileError {
    #[error("referenced log file cannot be opened safely")]
    Open(#[from] SafeOpenError),

    #[error("referenced log file has been rotated or replaced")]
    FileChanged,
}

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use tempfile::tempdir;

    use super::*;

    fn sample_data() -> MatchReferenceData {
        MatchReferenceData {
            source_id: "payment-test".to_owned(),
            relative_path: PathBuf::from("application.log"),
            file_identity: FileIdentity {
                device: 10,
                inode: 20,
            },
            line_number: 42,
            line_start_offset: 1024,
            match_byte_offset: 1050,
        }
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
    fn verifies_file_identity_before_context_reading() {
        let root_dir = tempdir().expect("temporary root should be created");
        let log_path = root_dir.path().join("application.log");
        fs::write(&log_path, "traceId=abc123\n").expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let reference = MatchReferenceData {
            file_identity: safe_file.identity(),
            ..sample_data()
        };

        let reopened = open_referenced_file(&root, &reference)
            .expect("unchanged referenced file should reopen");
        assert_eq!(reopened.identity(), reference.file_identity);
    }

    #[test]
    fn rejects_file_replaced_after_reference_creation() {
        let root_dir = tempdir().expect("temporary root should be created");
        let log_path = root_dir.path().join("application.log");
        let rotated_path = root_dir.path().join("application.log.1");
        fs::write(&log_path, "original\n").expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let reference = MatchReferenceData {
            file_identity: safe_file.identity(),
            ..sample_data()
        };

        fs::rename(&log_path, &rotated_path).expect("original file should rotate");
        fs::write(&log_path, "replacement\n").expect("replacement should be written");

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
