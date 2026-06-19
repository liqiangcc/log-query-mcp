use std::{
    collections::{HashMap, HashSet, VecDeque},
    os::unix::ffi::OsStrExt,
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
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
pub const MAX_CURSOR_CANDIDATE_FILES: usize = 500;
const MAX_RELATIVE_PATH_BYTES: usize = 4096;
const MAX_TIME_BOUND_CHARS: usize = 64;

/// Query parameters bound to a pagination cursor.
///
/// The client-visible `cursor` field is deliberately excluded. A continuation
/// request must otherwise reproduce the original search conditions exactly.
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
        if !is_normal_relative_path(&self.relative_path)
            || self.relative_path.as_os_str().as_bytes().len() > MAX_RELATIVE_PATH_BYTES
        {
            return Err(SearchCursorError::InvalidData(
                "candidate path must be a bounded normalized relative path",
            ));
        }
        Ok(())
    }
}

/// Server-internal continuation state. This type intentionally does not
/// implement `Serialize` so file paths, inode values and offsets cannot be
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
        if self.files_scanned != self.next_candidate_index {
            return Err(SearchCursorError::InvalidData(
                "files_scanned must equal the next candidate index",
            ));
        }

        let query_sources: HashSet<&str> =
            self.query.source_ids.iter().map(String::as_str).collect();
        let mut unique_candidates = HashSet::with_capacity(self.candidates.len());
        for candidate in &self.candidates {
            candidate.validate(&query_sources)?;
            if !unique_candidates
                .insert((candidate.source_id.clone(), candidate.relative_path.clone()))
            {
                return Err(SearchCursorError::InvalidData(
                    "candidate snapshot must not contain duplicate files",
                ));
            }
        }

        let current = self.current_candidate();
        if self.next_byte_offset > current.file_size_at_snapshot {
            return Err(SearchCursorError::InvalidData(
                "next byte offset exceeds the candidate snapshot",
            ));
        }
        if self.bytes_scanned < self.next_byte_offset {
            return Err(SearchCursorError::InvalidData(
                "cumulative scan bytes cannot precede the current byte offset",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn current_candidate(&self) -> &CursorCandidateFile {
        &self.candidates[self.next_candidate_index]
    }

    fn validate_continuation(&self, next: &Self) -> Result<(), SearchCursorError> {
        next.validate()
            .map_err(|_| SearchCursorError::InvalidContinuation("continuation state is invalid"))?;
        if next.query != self.query {
            return Err(SearchCursorError::InvalidContinuation(
                "continuation query differs from the original query",
            ));
        }
        if next.candidates != self.candidates {
            return Err(SearchCursorError::InvalidContinuation(
                "continuation candidate snapshot differs from the original snapshot",
            ));
        }
        if next.files_scanned < self.files_scanned
            || next.bytes_scanned < self.bytes_scanned
            || next.results_returned < self.results_returned
        {
            return Err(SearchCursorError::InvalidContinuation(
                "continuation counters cannot move backwards",
            ));
        }

        let progressed = next.next_candidate_index > self.next_candidate_index
            || (next.next_candidate_index == self.next_candidate_index
                && next.next_byte_offset > self.next_byte_offset);
        if !progressed {
            return Err(SearchCursorError::InvalidContinuation(
                "continuation position must move forward",
            ));
        }
        if next.next_candidate_index == self.next_candidate_index
            && next.next_line_number < self.next_line_number
        {
            return Err(SearchCursorError::InvalidContinuation(
                "continuation line number cannot move backwards within a file",
            ));
        }
        Ok(())
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
            if !state.evict_oldest_unleased() {
                return Err(SearchCursorError::CapacityBusy);
            }
        }
        Ok(state.insert_new(data, expires_at))
    }

    /// Leases a cursor for one page request.
    ///
    /// A leased cursor cannot be consumed concurrently. Dropping the lease
    /// without committing releases it for retry. Committing atomically removes
    /// the old token and optionally creates a fresh token for the next page.
    pub fn begin(
        self: &Arc<Self>,
        token: &str,
        expected_query: &SearchCursorQuery,
    ) -> Result<SearchCursorLease, SearchCursorError> {
        expected_query.validate()?;
        if !is_well_formed_token(token) {
            return Err(SearchCursorError::UnknownOrExpired);
        }

        let now = Instant::now();
        let lease_id = Uuid::new_v4();
        let data = {
            let mut state = self.lock_state();
            state.purge_expired(now);
            let stored = state
                .entries
                .get_mut(token)
                .ok_or(SearchCursorError::UnknownOrExpired)?;
            if &stored.data.query != expected_query {
                return Err(SearchCursorError::QueryMismatch);
            }
            if stored.lease_id.is_some() {
                return Err(SearchCursorError::Busy);
            }
            stored.lease_id = Some(lease_id);
            stored.data.clone()
        };

        Ok(SearchCursorLease {
            store: Arc::clone(self),
            token: token.to_owned(),
            lease_id,
            data,
            completed: false,
        })
    }

    pub fn begin_request(
        self: &Arc<Self>,
        token: &str,
        request: &SearchLogsRequest,
    ) -> Result<SearchCursorLease, SearchCursorError> {
        let query = SearchCursorQuery::from_request(request)?;
        self.begin(token, &query)
    }

    pub fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn finish_lease(
        &self,
        token: &str,
        lease_id: Uuid,
        current: &SearchCursorData,
        next: Option<SearchCursorData>,
    ) -> Result<Option<String>, SearchCursorError> {
        if let Some(next_data) = &next {
            current.validate_continuation(next_data)?;
        }

        let now = Instant::now();
        let next_expiration = if next.is_some() {
            Some(
                now.checked_add(self.ttl)
                    .ok_or(SearchCursorError::ExpirationOverflow)?,
            )
        } else {
            None
        };
        let mut state = self.lock_state();
        state.purge_expired(now);
        let stored = state
            .entries
            .get(token)
            .ok_or(SearchCursorError::LeaseLost)?;
        if stored.lease_id != Some(lease_id) || &stored.data != current {
            return Err(SearchCursorError::LeaseLost);
        }
        state.remove(token);

        let next_token = match (next, next_expiration) {
            (Some(data), Some(expires_at)) => Some(state.insert_new(data, expires_at)),
            (None, None) => None,
            _ => return Err(SearchCursorError::LeaseLost),
        };
        Ok(next_token)
    }

    fn release_lease(&self, token: &str, lease_id: Uuid) {
        let now = Instant::now();
        let mut state = self.lock_state();
        let remove_expired = if let Some(stored) = state.entries.get_mut(token) {
            if stored.lease_id != Some(lease_id) {
                false
            } else {
                stored.lease_id = None;
                stored.expires_at <= now
            }
        } else {
            false
        };
        if remove_expired {
            state.remove(token);
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, CursorStoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub struct SearchCursorLease {
    store: Arc<SearchCursorStore>,
    token: String,
    lease_id: Uuid,
    data: SearchCursorData,
    completed: bool,
}

impl SearchCursorLease {
    #[must_use]
    pub fn data(&self) -> &SearchCursorData {
        &self.data
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn commit(
        mut self,
        next: Option<SearchCursorData>,
    ) -> Result<Option<String>, SearchCursorError> {
        let result = self
            .store
            .finish_lease(&self.token, self.lease_id, &self.data, next);
        if result.is_ok() {
            self.completed = true;
        }
        result
    }
}

impl Drop for SearchCursorLease {
    fn drop(&mut self) {
        if !self.completed {
            self.store.release_lease(&self.token, self.lease_id);
        }
    }
}

#[derive(Debug, Default)]
struct CursorStoreState {
    entries: HashMap<String, StoredCursor>,
    order: VecDeque<String>,
}

impl CursorStoreState {
    fn insert_new(&mut self, data: SearchCursorData, expires_at: Instant) -> String {
        let token = new_unique_token(&self.entries);
        self.order.push_back(token.clone());
        self.entries.insert(
            token.clone(),
            StoredCursor {
                data,
                expires_at,
                lease_id: None,
            },
        );
        token
    }

    fn purge_expired(&mut self, now: Instant) -> usize {
        let before = self.entries.len();
        let mut retained = VecDeque::with_capacity(self.order.len());
        while let Some(token) = self.order.pop_front() {
            match self.entries.get(&token) {
                Some(stored) if stored.expires_at <= now && stored.lease_id.is_none() => {
                    self.entries.remove(&token);
                }
                Some(_) => retained.push_back(token),
                None => {}
            }
        }
        self.order = retained;
        before - self.entries.len()
    }

    fn evict_oldest_unleased(&mut self) -> bool {
        let candidates = self.order.len();
        for _ in 0..candidates {
            let Some(token) = self.order.pop_front() else {
                break;
            };
            match self.entries.get(&token) {
                Some(stored) if stored.lease_id.is_some() => self.order.push_back(token),
                Some(_) => {
                    self.entries.remove(&token);
                    return true;
                }
                None => {}
            }
        }
        false
    }

    fn remove(&mut self, token: &str) {
        self.entries.remove(token);
        self.order.retain(|candidate| candidate != token);
    }
}

#[derive(Debug)]
struct StoredCursor {
    data: SearchCursorData,
    expires_at: Instant,
    lease_id: Option<Uuid>,
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

fn is_normal_relative_path(path: &Path) -> bool {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => return false,
        }
    }
    has_component
}

fn validate_time_bound(value: Option<&str>) -> Result<(), SearchCursorError> {
    if value.is_some_and(|bound| bound.is_empty() || bound.chars().count() > MAX_TIME_BOUND_CHARS) {
        return Err(SearchCursorError::InvalidData(
            "time bound length is outside the server limit",
        ));
    }
    Ok(())
}

/// Reopens the current snapshot file and verifies its identity and captured size.
///
/// Appends are allowed, but callers must never read beyond
/// `file_size_at_snapshot`; files created or bytes appended after the first page
/// are outside this cursor's stable snapshot.
pub fn open_cursor_file(
    root: &SafeRoot,
    cursor: &SearchCursorData,
) -> Result<SafeFile, SearchCursorFileError> {
    cursor.validate()?;
    let candidate = cursor.current_candidate();
    let safe_file = root.open_regular_file(&candidate.relative_path)?;
    if safe_file.identity() != candidate.file_identity {
        return Err(SearchCursorFileError::FileChanged);
    }
    if safe_file.size() < candidate.file_size_at_snapshot {
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

    #[error("invalid search cursor continuation: {0}")]
    InvalidContinuation(&'static str),

    #[error("unknown or expired search cursor")]
    UnknownOrExpired,

    #[error("search cursor does not belong to these query parameters")]
    QueryMismatch,

    #[error("search cursor is already being consumed")]
    Busy,

    #[error("search cursor store is full of active page requests")]
    CapacityBusy,

    #[error("search cursor lease was lost or expired")]
    LeaseLost,
}

#[derive(Debug, Error)]
pub enum SearchCursorFileError {
    #[error("search cursor data is invalid")]
    InvalidCursor(#[from] SearchCursorError),

    #[error("cursor log file cannot be opened safely")]
    Open(#[from] SafeOpenError),

    #[error("cursor log file has been rotated or replaced")]
    FileChanged,

    #[error("cursor log file was truncated below its captured snapshot size")]
    FileTruncated,
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Write, sync::Arc, thread};

    use tempfile::tempdir;

    use super::*;

    fn query() -> SearchCursorQuery {
        SearchCursorQuery {
            source_ids: vec!["payment-test".to_owned(), "order-test".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results: 50,
        }
    }

    fn candidates() -> Vec<CursorCandidateFile> {
        vec![
            CursorCandidateFile {
                source_id: "payment-test".to_owned(),
                relative_path: PathBuf::from("application.log"),
                file_identity: FileIdentity {
                    device: 10,
                    inode: 20,
                },
                file_size_at_snapshot: 100,
            },
            CursorCandidateFile {
                source_id: "order-test".to_owned(),
                relative_path: PathBuf::from("application.log"),
                file_identity: FileIdentity {
                    device: 11,
                    inode: 21,
                },
                file_size_at_snapshot: 200,
            },
        ]
    }

    fn data() -> SearchCursorData {
        SearchCursorData {
            query: query(),
            candidates: candidates(),
            next_candidate_index: 0,
            next_byte_offset: 20,
            next_line_number: 2,
            files_scanned: 0,
            bytes_scanned: 20,
            results_returned: 5,
        }
    }

    fn advanced_data() -> SearchCursorData {
        SearchCursorData {
            next_byte_offset: 60,
            next_line_number: 4,
            bytes_scanned: 60,
            results_returned: 10,
            ..data()
        }
    }

    #[test]
    fn creates_opaque_cursor_and_binds_query() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("cursor should begin");

        assert!(token.starts_with(SEARCH_CURSOR_PREFIX));
        assert_eq!(token.len(), SEARCH_CURSOR_LENGTH);
        assert!(!token.contains("traceId"));
        assert!(!token.contains("application"));
        assert_eq!(lease.data(), &data());
    }

    #[test]
    fn rejects_cursor_with_changed_query_conditions() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let mut changed_query = query();
        changed_query.keyword = "orderId=10001".to_owned();

        assert!(matches!(
            store.begin(&token, &changed_query),
            Err(SearchCursorError::QueryMismatch)
        ));
    }

    #[test]
    fn prevents_concurrent_consumption_and_drop_releases_lease() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let lease = store
            .begin(&token, &query())
            .expect("first lease should begin");

        assert!(matches!(
            store.begin(&token, &query()),
            Err(SearchCursorError::Busy)
        ));
        drop(lease);
        assert!(store.begin(&token, &query()).is_ok());
    }

    #[test]
    fn commit_invalidates_previous_cursor_atomically() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("cursor should begin");
        let next_token = lease
            .commit(Some(advanced_data()))
            .expect("continuation should commit")
            .expect("next cursor should be returned");

        assert_ne!(next_token, token);
        assert!(matches!(
            store.begin(&token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
        let next_lease = store
            .begin(&next_token, &query())
            .expect("next cursor should begin");
        assert_eq!(next_lease.data().next_byte_offset, 60);
    }

    #[test]
    fn completing_last_page_consumes_cursor_without_replacement() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("cursor should begin");

        assert_eq!(lease.commit(None).expect("cursor should complete"), None);
        assert!(store.is_empty());
    }

    #[test]
    fn invalid_continuation_releases_original_cursor_for_retry() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store.insert(data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("cursor should begin");
        let mut invalid = advanced_data();
        invalid.next_byte_offset = 10;

        assert!(matches!(
            lease.commit(Some(invalid)),
            Err(SearchCursorError::InvalidContinuation(_))
        ));
        assert!(store.begin(&token, &query()).is_ok());
    }

    #[test]
    fn capacity_does_not_evict_an_active_cursor() {
        let store = Arc::new(
            SearchCursorStore::new(1, Duration::from_secs(60)).expect("store should be created"),
        );
        let first = store.insert(data()).expect("cursor should be inserted");
        let lease = store.begin(&first, &query()).expect("cursor should begin");

        assert!(matches!(
            store.insert(data()),
            Err(SearchCursorError::CapacityBusy)
        ));
        drop(lease);
        let second = store
            .insert(data())
            .expect("unleased cursor can be evicted");
        assert_ne!(first, second);
    }

    #[test]
    fn expires_and_evicts_cursor_state() {
        let expiring = Arc::new(
            SearchCursorStore::new(10, Duration::from_millis(5)).expect("store should be created"),
        );
        let token = expiring.insert(data()).expect("cursor should be inserted");
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            expiring.begin(&token, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));

        let bounded = Arc::new(
            SearchCursorStore::new(1, Duration::from_secs(60)).expect("store should be created"),
        );
        let first = bounded.insert(data()).expect("cursor should be inserted");
        let second = bounded
            .insert(data())
            .expect("second cursor should be inserted");
        assert!(matches!(
            bounded.begin(&first, &query()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
        assert!(bounded.begin(&second, &query()).is_ok());
    }

    #[test]
    fn new_store_invalidates_existing_cursor() {
        let first_store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = first_store
            .insert(data())
            .expect("cursor should be inserted");
        let new_store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60))
                .expect("new store should be created"),
        );

        assert!(matches!(
            new_store.begin(&token, &query()),
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
        invalid.candidates[0].source_id = "unknown-test".to_owned();
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

        let mut invalid = data();
        invalid.files_scanned = 1;
        assert!(matches!(
            invalid.validate(),
            Err(SearchCursorError::InvalidData(_))
        ));

        let mut invalid = data();
        invalid.candidates.push(invalid.candidates[0].clone());
        assert!(matches!(
            invalid.validate(),
            Err(SearchCursorError::InvalidData(_))
        ));
    }

    #[test]
    fn continuation_cannot_change_snapshot_or_move_backwards() {
        let current = data();
        let mut changed_snapshot = advanced_data();
        changed_snapshot.candidates[0].file_size_at_snapshot += 1;
        assert!(matches!(
            current.validate_continuation(&changed_snapshot),
            Err(SearchCursorError::InvalidContinuation(_))
        ));

        let mut backwards = advanced_data();
        backwards.bytes_scanned = 10;
        assert!(matches!(
            current.validate_continuation(&backwards),
            Err(SearchCursorError::InvalidContinuation(_))
        ));
    }

    #[test]
    fn file_snapshot_allows_append_but_rejects_replace_and_truncate() {
        let root_dir = tempdir().expect("temporary root should be created");
        let path = root_dir.path().join("application.log");
        fs::write(&path, b"first page\n").expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("file should open");
        let cursor = SearchCursorData {
            query: SearchCursorQuery {
                source_ids: vec!["payment-test".to_owned()],
                ..query()
            },
            candidates: vec![CursorCandidateFile {
                source_id: "payment-test".to_owned(),
                relative_path: PathBuf::from("application.log"),
                file_identity: safe_file.identity(),
                file_size_at_snapshot: safe_file.size(),
            }],
            next_candidate_index: 0,
            next_byte_offset: 0,
            next_line_number: 1,
            files_scanned: 0,
            bytes_scanned: 0,
            results_returned: 0,
        };

        let mut append = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("fixture should open for append");
        append
            .write_all(b"appended\n")
            .expect("fixture should append");
        assert!(open_cursor_file(&root, &cursor).is_ok());

        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .expect("fixture should open for truncation")
            .set_len(4)
            .expect("fixture should truncate");
        assert!(matches!(
            open_cursor_file(&root, &cursor),
            Err(SearchCursorFileError::FileTruncated)
        ));

        let rotated = root_dir.path().join("application.log.1");
        fs::rename(&path, rotated).expect("fixture should rotate");
        fs::write(&path, b"replacement content\n").expect("replacement should be written");
        assert!(matches!(
            open_cursor_file(&root, &cursor),
            Err(SearchCursorFileError::FileChanged)
        ));
    }

    #[test]
    fn query_is_derived_from_request_without_embedding_cursor_token() {
        let request = SearchLogsRequest {
            source_ids: vec!["payment-test".to_owned(), "order-test".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results: 50,
            cursor: Some("cur_placeholder".to_owned()),
        };
        let derived = SearchCursorQuery::from_request(&request).expect("query should be derived");

        assert_eq!(derived, query());
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
