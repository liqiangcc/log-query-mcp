use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Component, Path, PathBuf},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use thiserror::Error;
use uuid::Uuid;

use crate::{
    FileIdentity, MAX_KEYWORD_CHARS, MAX_RESULTS, MAX_SOURCE_ID_CHARS, MAX_SOURCES, ResultOrder,
    SafeFile, SafeOpenError, SafeRoot, SearchLogsRequest,
};

const SEARCH_CURSOR_PREFIX: &str = "cur_";
const SEARCH_CURSOR_LENGTH: usize = SEARCH_CURSOR_PREFIX.len() + 32;
pub const MAX_CURSOR_CANDIDATE_FILES: usize = 10_000;

/// Query parameters bound to a pagination cursor.
///
/// The cursor store compares this value before returning continuation state, so
/// a token cannot be reused with a different source list, keyword, time range,
/// result order or page size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCursorQuery {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub order: ResultOrder,
    pub max_results: usize,
}

impl SearchCursorQuery {
    pub fn from_request(request: &SearchLogsRequest) -> Result<Self, SearchCursorError> {
        let query = Self {
            source_ids: request.source_ids.clone(),
            keyword: request.keyword.clone(),
            case_sensitive: request.case_sensitive,
            start_time: request.start_time.clone(),
            end_time: request.end_time.clone(),
            order: request.order,
            max_results: request.max_results,
        };
        query.validate()?;
        Ok(query)
    }

    pub fn validate(&self) -> Result<(), SearchCursorError> {
        if self.source_ids.is_empty() || self.source_ids.len() > MAX_SOURCES {
            return Err(SearchCursorError::InvalidData(
                "source_ids count is outside the server limit",
            ));
        }

        let mut unique_sources = HashSet::with_capacity(self.source_ids.len());
        for source_id in &self.source_ids {
            let chars = source_id.chars().count();
            if chars == 0 || chars > MAX_SOURCE_ID_CHARS {
                return Err(SearchCursorError::InvalidData(
                    "source_id length is outside the server limit",
                ));
            }
            if !unique_sources.insert(source_id) {
                return Err(SearchCursorError::InvalidData(
                    "source_ids must not contain duplicates",
                ));
            }
        }

        let keyword_bytes = self.keyword.as_bytes();
        if keyword_bytes.is_empty()
            || self.keyword.chars().count() > MAX_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(SearchCursorError::InvalidData(
                "keyword is not a valid literal log search term",
            ));
        }

        if self.max_results == 0 || self.max_results > MAX_RESULTS {
            return Err(SearchCursorError::InvalidData(
                "max_results is outside the server limit",
            ));
        }

        validate_time_bound(self.start_time.as_deref())?;
        validate_time_bound(self.end_time.as_deref())?;

        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorCandidateFile {
    pub source_id: String,
    pub relative_path: PathBuf,
    pub file_identity: FileIdentity,
    pub file_size_at_snapshot: u64,
}

impl CursorCandidateFile {
    fn validate(&self, query_sources: &HashSet<&str>) -> Result<(), SearchCursorError> {
        if !query_sources.contains(self.source_id.as_str()) {
            return Err(SearchCursorError::InvalidData(
                "candidate file source is not part of the query",
            ));
        }
        validate_relative_path(&self.relative_path)
    }
}

/// Server-internal continuation state. This type intentionally does not
/// implement Serialize so file paths, inode values and offsets cannot be
/// emitted as MCP structured content by accident.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCursorData {
    pub query: SearchCursorQuery,
    pub candidates: Vec<CursorCandidateFile>,
    pub next_candidate_index: usize,
    pub next_byte_offset: u64,
    pub next_line_number: u64,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub results_returned: usize,
}

impl SearchCursorData {
    pub fn validate(&self) -> Result<(), SearchCursorError> {
        self.query.validate()?;
        if self.candidates.is_empty() || self.candidates.len() > MAX_CURSOR_CANDIDATE_FILES {
            return Err(SearchCursorError::InvalidData(
                "candidate file count is outside the server limit",
            ));
        }
        if self.next_candidate_index >= self.candidates.len() {
            return Err(SearchCursorError::InvalidData(
                "next candidate index must identify an unfinished file",
            ));
        }
        if self.next_line_number == 0 {
            return Err(SearchCursorError::InvalidData(
                "next line number must start at one",
            ));
        }

        let query_sources: HashSet<&str> =
            self.query.source_ids.iter().map(String::as_str).collect();
        for candidate in &self.candidates {
            candidate.validate(&query_sources)?;
        }

        let current = &self.candidates[self.next_candidate_index];
        if self.next_byte_offset > current.file_size_at_snapshot {
            return Err(SearchCursorError::InvalidData(
                "next byte offset exceeds the candidate snapshot",
            ));
        }
        if self.files_scanned > self.candidates.len() {
            return Err(SearchCursorError::InvalidData(
                "files_scanned exceeds the candidate snapshot",
            ));
        }

        Ok(())
    }

    #[must_use]
    pub fn current_candidate(&self) -> &CursorCandidateFile {
        &self.candidates[self.next_candidate_index]
    }
}

#[derive(Debug)]
pub struct SearchCursorStore {
    capacity: usize,
    ttl: Duration,
    state: Mutex<CursorStoreState>,
}

impl SearchCursorStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, SearchCursorError> {
        if capacity == 0 {
            return Err(SearchCursorError::InvalidCapacity);
        }
        if ttl == Duration::ZERO {
            return Err(SearchCursorError::InvalidTtl);
        }

        Ok(Self {
            capacity,
            ttl,
            state: Mutex::new(CursorStoreState::default()),
        })
    }

    pub fn insert(&self, data: SearchCursorData) -> Result<String, SearchCursorError> {
        data.validate()?;
        let now = Instant::now();
        let expires_at = now
            .checked_add(self.ttl)
            .ok_or(SearchCursorError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }

        let token = new_unique_token(&state.entries);
        state.order.push_back(token.clone());
        state.entries.insert(
            token.clone(),
            StoredCursor {
                data,
                expires_at,
            },
        );
        Ok(token)
    }

    pub fn resolve(
        &self,
        token: &str,
        expected_query: &SearchCursorQuery,
    ) -> Result<SearchCursorData, SearchCursorError> {
        expected_query.validate()?;
        if !is_well_formed_token(token) {
            return Err(SearchCursorError::UnknownOrExpired);
        }

        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        let stored = state
            .entries
            .get(token)
            .ok_or(SearchCursorError::UnknownOrExpired)?;
        if &stored.data.query != expected_query {
            return Err(SearchCursorError::QueryMismatch);
        }

        Ok(stored.data.clone())
    }

    /// Atomically invalidates the previous token and creates a token for the
    /// next continuation position after a page has been produced successfully.
    pub fn replace(
        &self,
        token: &str,
        expected_query: &SearchCursorQuery,
        next_data: SearchCursorData,
    ) -> Result<String, SearchCursorError> {
        expected_query.validate()?;
        next_data.validate()?;
        if next_data.query != *expected_query {
            return Err(SearchCursorError::QueryMismatch);
        }
        if !is_well_formed_token(token) {
            return Err(SearchCursorError::UnknownOrExpired);
        }

        let now = Instant::now();
        let expires_at = now
            .checked_add(self.ttl)
            .ok_or(SearchCursorError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        let stored = state
            .entries
            .get(token)
            .ok_or(SearchCursorError::UnknownOrExpired)?;
        if &stored.data.query != expected_query {
            return Err(SearchCursorError::QueryMismatch);
        }

        state.entries.remove(token);
        let next_token = new_unique_token(&state.entries);
        state.order.push_back(next_token.clone());
        state.entries.insert(
            next_token.clone(),
            StoredCursor {
                data: next_data,
                expires_at,
            },
        );
        Ok(next_token)
    }

    pub fn complete(
        &self,
        token: &str,
        expected_query: &SearchCursorQuery,
    ) -> Result<(), SearchCursorError> {
        expected_query.validate()?;
        if !is_well_formed_token(token) {
            return Err(SearchCursorError::UnknownOrExpired);
        }

        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        let stored = state
            .entries
            .get(token)
            .ok_or(SearchCursorError::UnknownOrExpired)?;
        if &stored.data.query != expected_query {
            return Err(SearchCursorError::QueryMismatch);
        }
        state.entries.remove(token);
        Ok(())
    }

    pub fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock_state(&self) -> MutexGuard<'_, CursorStoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Default)]
struct CursorStoreState {
    entries: HashMap<String, StoredCursor>,
    order: VecDeque<String>,
}

impl CursorStoreState {
    fn purge_expired(&mut self, now: Instant) {
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
struct StoredCursor {
    data: SearchCursorData,
    expires_at: Instant,
}

fn new_unique_token(entries: &HashMap<String, StoredCursor>) -> String {
    loop {
        let candidate = format!("{SEARCH_CURSOR_PREFIX}{}", Uuid::new_v4().simple());
        if !entries.contains_key(&candidate) {
            return candidate;
        }
    }
}

fn is_well_formed_token(token: &str) -> bool {
    token.len() == SEARCH_CURSOR_LENGTH
        && token.starts_with(SEARCH_CURSOR_PREFIX)
        && token[SEARCH_CURSOR_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

fn validate_relative_path(path: &Path) -> Result<(), SearchCursorError> {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(SearchCursorError::InvalidData(
                    "candidate path must be a normalized relative path",
                ));
            }
        }
    }
    if !has_component {
        return Err(SearchCursorError::InvalidData(
            "candidate path must not be empty",
        ));
    }
    Ok(())
}

fn validate_time_bound(value: Option<&str>) -> Result<(), SearchCursorError> {
    if value.is_some_and(|value| value.is_empty() || value.len() > 64) {
        return Err(SearchCursorError::InvalidData(
            "time bound length is outside the server limit",
        ));
    }
    Ok(())
}

pub fn open_cursor_file(
    root: &SafeRoot,
    cursor: &SearchCursorData,
) -> Result<SafeFile, SearchCursorFileError> {
    let candidate = cursor.current_candidate();
    let safe_file = root.open_regular_file(&candidate.relative_path)?;
    if safe_file.identity() != candidate.file_identity {
        return Err(SearchCursorFileError::FileChanged);
    }
    if safe_file.size() < cursor.next_byte_offset {
        return Err(SearchCursorFileError::FileTruncated);
    }
    Ok(safe_file)
}

#[derive(Debug, Error)]
pub enum SearchCursorError {
    #[error("search cursor capacity must be greater than zero")]
    InvalidCapacity,

    #[error("search cursor TTL must be greater than zero")]
    InvalidTtl,

    #[error("search cursor expiration cannot be represented")]
    ExpirationOverflow,

    #[error("invalid search cursor data: {0}")]
    InvalidData(&'static str),

    #[error("unknown or expired search cursor")]
    UnknownOrExpired,

    #[error("search cursor does not belong to these query parameters")]
    QueryMismatch,
}

#[derive(Debug, Error)]
pub enum SearchCursorFileError {
    #[error("cursor log file cannot be opened safely")]
    Open(#[from] SafeOpenError),

    #[error("cursor log file has been rotated or replaced")]
    FileChanged,

    #[error("cursor log file was truncated before the continuation offset")]
    FileTruncated,
}

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use tempfile::tempdir;

    use super::*;

    fn query() -> SearchCursorQuery {
        SearchCursorQuery {
            source_ids: vec!["payment-test".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results: 50,
        }
    }

    fn candidate(identity: FileIdentity, size: u64) -> CursorCandidateFile {
        CursorCandidateFile {
            source_id: "payment-test".to_owned(),
            relative_path: PathBuf::from("application.log"),
            file_identity: identity,
            file_size_at_snapshot: size,
        }
    }

    fn data() -> SearchCursorData {
        SearchCursorData {
            query: query(),
            candidates: vec![candidate(
                FileIdentity {
                    device: 10,
                    inode: 20,
                },
                4096,
            )],
            next_candidate_index: 0,
            next_byte_offset: 1024,
            next_line_number: 42,
            files_scanned: 0,
            bytes_scanned: 1024,
            results_returned: 50,
        }
    }

    #[test]
    fn creates_opaque_cursor_and_resolves_state() {
        let store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(data()).expect("cursor should be inserted");

        assert!(token.starts_with(SEARCH_CURSOR_PREFIX));
        assert_eq!(token.len(), SEARCH_CURSOR_LENGTH);
        assert!(!token.contains("payment"));
        assert!(!token.contains("application"));
        assert_eq!(store.resolve(&token, &query()).expect("cursor should resolve"), data());
    }

    #[test]
    fn rejects_cursor_with_changed_query_conditions() {
        let store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(data()).expect("cursor should be inserted");
        let mut changed_query = query();
        changed_query.keyword = "orderId=10001".to_owned();

        assert!(matches!(
            store.resolve(&token, &changed_query),
            Err(SearchCursorError::QueryMismatch)
        ));
    }

    #[test]
    fn replace_invalidates_previous_cursor_atomically() {
        let store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(data()).expect("cursor should be inserted");
        let mut next_data = data();
        next_data.next_byte_offset = 2048;
        next_data.next_line_number = 80;
        let next_token = store
            .replace(&token, &query(), next_data.clone())
            .expect("cursor should be replaced");

        assert_ne!(next_token, token);
        assert!(matches!(
            store.resolve(&token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
        assert_eq!(
            store
                .resolve(&next_token, &query())
                .expect("next cursor should resolve"),
            next_data
        );
    }

    #[test]
    fn complete_removes_cursor() {
        let store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(data()).expect("cursor should be inserted");
        store
            .complete(&token, &query())
            .expect("cursor should complete");

        assert!(store.is_empty());
        assert!(matches!(
            store.resolve(&token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
    }

    #[test]
    fn expires_and_evicts_cursor_state() {
        let expiring = SearchCursorStore::new(10, Duration::from_millis(5))
            .expect("store should be created");
        let expiring_token = expiring
            .insert(data())
            .expect("cursor should be inserted");
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            expiring.resolve(&expiring_token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));

        let bounded = SearchCursorStore::new(1, Duration::from_secs(60))
            .expect("store should be created");
        let first = bounded.insert(data()).expect("cursor should be inserted");
        let second = bounded
            .insert(data())
            .expect("second cursor should be inserted");
        assert!(matches!(
            bounded.resolve(&first, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
        assert!(bounded.resolve(&second, &query()).is_ok());
    }

    #[test]
    fn new_store_invalidates_existing_cursor() {
        let first_store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = first_store
            .insert(data())
            .expect("cursor should be inserted");
        let new_store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("new store should be created");

        assert!(matches!(
            new_store.resolve(&token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
    }

    #[test]
    fn validates_cursor_data_and_candidate_boundaries() {
        let mut invalid = data();
        invalid.candidates[0].relative_path = PathBuf::from("../secret.log");
        assert!(matches!(
            invalid.validate(),
            Err(SearchCursorError::InvalidData(_))
        ));

        let mut invalid = data();
        invalid.candidates[0].source_id = "order-test".to_owned();
        assert!(matches!(
            invalid.validate(),
            Err(SearchCursorError::InvalidData(_))
        ));

        let mut invalid = data();
        invalid.next_byte_offset = 5000;
        assert!(matches!(
            invalid.validate(),
            Err(SearchCursorError::InvalidData(_))
        ));
    }

    #[test]
    fn safely_reopens_unchanged_cursor_file() {
        let root_dir = tempdir().expect("temporary root should be created");
        let path = root_dir.path().join("application.log");
        fs::write(&path, vec![b'x'; 2048]).expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let cursor = SearchCursorData {
            candidates: vec![candidate(safe_file.identity(), safe_file.size())],
            next_byte_offset: 1024,
            ..data()
        };

        let reopened = open_cursor_file(&root, &cursor).expect("file should reopen");
        assert_eq!(reopened.identity(), safe_file.identity());
    }

    #[test]
    fn rejects_replaced_or_truncated_cursor_file() {
        let root_dir = tempdir().expect("temporary root should be created");
        let path = root_dir.path().join("application.log");
        let rotated = root_dir.path().join("application.log.1");
        fs::write(&path, vec![b'x'; 2048]).expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let cursor = SearchCursorData {
            candidates: vec![candidate(safe_file.identity(), safe_file.size())],
            next_byte_offset: 1024,
            ..data()
        };

        fs::rename(&path, &rotated).expect("file should rotate");
        fs::write(&path, vec![b'y'; 2048]).expect("replacement should be written");
        assert!(matches!(
            open_cursor_file(&root, &cursor),
            Err(SearchCursorFileError::FileChanged)
        ));

        fs::remove_file(&path).expect("replacement should be removed");
        fs::rename(&rotated, &path).expect("original inode should be restored");
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("file should open for truncation")
            .set_len(512)
            .expect("file should truncate");
        assert!(matches!(
            open_cursor_file(&root, &cursor),
            Err(SearchCursorFileError::FileTruncated)
        ));
    }

    #[test]
    fn validates_store_configuration() {
        assert!(matches!(
            SearchCursorStore::new(0, Duration::from_secs(1)),
            Err(SearchCursorError::InvalidCapacity)
        ));
        assert!(matches!(
            SearchCursorStore::new(1, Duration::ZERO),
            Err(SearchCursorError::InvalidTtl)
        ));
    }
}
