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

const CURSOR_PREFIX: &str = "cur_";
const CURSOR_LENGTH: usize = CURSOR_PREFIX.len() + 32;
const MAX_CURSOR_FILES: usize = 500;
const MAX_RELATIVE_PATH_BYTES: usize = 4096;
const MAX_TIME_BOUND_CHARS: usize = 64;

/// Immutable search conditions bound to every page cursor.
///
/// The client-visible `cursor` field is deliberately excluded. A continuation
/// request must otherwise reproduce the original query exactly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorQuery {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub order: ResultOrder,
    pub max_results: usize,
}

impl CursorQuery {
    pub fn from_request(request: &SearchLogsRequest) -> Result<Self, QueryCursorError> {
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

    pub fn validate(&self) -> Result<(), QueryCursorError> {
        if self.source_ids.is_empty() || self.source_ids.len() > MAX_SOURCES {
            return Err(QueryCursorError::InvalidData(
                "source_ids count is outside the server limit",
            ));
        }
        let mut unique_sources = HashSet::with_capacity(self.source_ids.len());
        for source_id in &self.source_ids {
            let chars = source_id.chars().count();
            if chars == 0 || chars > MAX_SOURCE_ID_CHARS {
                return Err(QueryCursorError::InvalidData(
                    "source_id length is outside the server limit",
                ));
            }
            if !unique_sources.insert(source_id) {
                return Err(QueryCursorError::InvalidData(
                    "source_ids cannot contain duplicates",
                ));
            }
        }

        let keyword_bytes = self.keyword.as_bytes();
        if keyword_bytes.is_empty()
            || self.keyword.chars().count() > MAX_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(QueryCursorError::InvalidData(
                "keyword is not a valid literal search term",
            ));
        }
        if self.max_results == 0 || self.max_results > MAX_RESULTS {
            return Err(QueryCursorError::InvalidData(
                "max_results is outside the server limit",
            ));
        }
        validate_time_bound(self.start_time.as_deref())?;
        validate_time_bound(self.end_time.as_deref())?;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorFileSnapshot {
    pub source_id: String,
    pub relative_path: PathBuf,
    pub file_identity: FileIdentity,
    pub file_size: u64,
}

impl CursorFileSnapshot {
    fn validate(&self, query: &CursorQuery) -> Result<(), QueryCursorError> {
        if !query.source_ids.contains(&self.source_id) {
            return Err(QueryCursorError::InvalidData(
                "cursor file source is not part of the bound query",
            ));
        }
        if !is_normal_relative_path(&self.relative_path)
            || self.relative_path.as_os_str().as_bytes().len() > MAX_RELATIVE_PATH_BYTES
        {
            return Err(QueryCursorError::InvalidData(
                "cursor file path must be a bounded normalized relative path",
            ));
        }
        Ok(())
    }
}

/// Location at which the next page should resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CursorPosition {
    pub file_index: usize,
    pub byte_offset: u64,
    pub line_number: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryCursorData {
    pub query: CursorQuery,
    pub files: Vec<CursorFileSnapshot>,
    pub position: CursorPosition,
    pub cumulative_scanned_bytes: u64,
    pub cumulative_returned_results: u64,
}

impl QueryCursorData {
    pub fn validate(&self) -> Result<(), QueryCursorError> {
        self.query.validate()?;
        if self.files.is_empty() || self.files.len() > MAX_CURSOR_FILES {
            return Err(QueryCursorError::InvalidData(
                "cursor file snapshot count is outside the server limit",
            ));
        }

        let mut unique_files = HashSet::with_capacity(self.files.len());
        for file in &self.files {
            file.validate(&self.query)?;
            if !unique_files.insert((file.source_id.clone(), file.relative_path.clone())) {
                return Err(QueryCursorError::InvalidData(
                    "cursor file snapshot contains duplicates",
                ));
            }
        }

        let Some(current_file) = self.files.get(self.position.file_index) else {
            return Err(QueryCursorError::InvalidData(
                "cursor position must identify an incomplete file snapshot",
            ));
        };
        if self.position.line_number == 0 {
            return Err(QueryCursorError::InvalidData(
                "cursor line number must start at one",
            ));
        }
        if self.position.byte_offset > current_file.file_size {
            return Err(QueryCursorError::InvalidData(
                "cursor byte offset exceeds the captured file size",
            ));
        }
        Ok(())
    }

    fn validate_continuation(&self, next: &Self) -> Result<(), QueryCursorError> {
        next.validate()?;
        if next.query != self.query {
            return Err(QueryCursorError::InvalidContinuation(
                "continuation query differs from the original query",
            ));
        }
        if next.files != self.files {
            return Err(QueryCursorError::InvalidContinuation(
                "continuation file snapshot differs from the original snapshot",
            ));
        }
        if next.cumulative_scanned_bytes < self.cumulative_scanned_bytes
            || next.cumulative_returned_results < self.cumulative_returned_results
        {
            return Err(QueryCursorError::InvalidContinuation(
                "continuation counters cannot move backwards",
            ));
        }

        let progressed = next.position.file_index > self.position.file_index
            || (next.position.file_index == self.position.file_index
                && next.position.byte_offset > self.position.byte_offset);
        if !progressed {
            return Err(QueryCursorError::InvalidContinuation(
                "continuation position must move forward",
            ));
        }
        if next.position.file_index == self.position.file_index
            && next.position.line_number < self.position.line_number
        {
            return Err(QueryCursorError::InvalidContinuation(
                "continuation line number cannot move backwards within a file",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct QueryCursorStore {
    capacity: usize,
    ttl: Duration,
    state: Mutex<StoreState>,
}

impl QueryCursorStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, QueryCursorError> {
        if capacity == 0 {
            return Err(QueryCursorError::InvalidCapacity);
        }
        if ttl == Duration::ZERO {
            return Err(QueryCursorError::InvalidTtl);
        }
        Ok(Self {
            capacity,
            ttl,
            state: Mutex::new(StoreState::default()),
        })
    }

    pub fn insert(&self, data: QueryCursorData) -> Result<String, QueryCursorError> {
        data.validate()?;
        let now = Instant::now();
        let expires_at = now
            .checked_add(self.ttl)
            .ok_or(QueryCursorError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }
        Ok(state.insert_new(data, expires_at))
    }

    pub fn begin(
        self: &Arc<Self>,
        token: &str,
        query: &CursorQuery,
    ) -> Result<QueryCursorLease, QueryCursorError> {
        query.validate()?;
        if !is_well_formed_token(token) {
            return Err(QueryCursorError::UnknownOrExpired);
        }

        let now = Instant::now();
        let lease_id = Uuid::new_v4();
        let data = {
            let mut state = self.lock_state();
            state.purge_expired(now);
            let entry = state
                .entries
                .get_mut(token)
                .ok_or(QueryCursorError::UnknownOrExpired)?;
            if &entry.data.query != query {
                return Err(QueryCursorError::QueryMismatch);
            }
            if entry.lease_id.is_some() {
                return Err(QueryCursorError::Busy);
            }
            entry.lease_id = Some(lease_id);
            entry.data.clone()
        };

        Ok(QueryCursorLease {
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
    ) -> Result<QueryCursorLease, QueryCursorError> {
        let query = CursorQuery::from_request(request)?;
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
        current: &QueryCursorData,
        next: Option<QueryCursorData>,
    ) -> Result<Option<String>, QueryCursorError> {
        if let Some(next_data) = &next {
            current.validate_continuation(next_data)?;
        }
        let now = Instant::now();
        let next_expiration = now
            .checked_add(self.ttl)
            .ok_or(QueryCursorError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);

        let entry = state
            .entries
            .get(token)
            .ok_or(QueryCursorError::LeaseLost)?;
        if entry.lease_id != Some(lease_id) || &entry.data != current {
            return Err(QueryCursorError::LeaseLost);
        }
        state.remove(token);

        let next_token = next.map(|data| {
            while state.entries.len() >= self.capacity {
                state.evict_oldest();
            }
            state.insert_new(data, next_expiration)
        });
        Ok(next_token)
    }

    fn release_lease(&self, token: &str, lease_id: Uuid) {
        let mut state = self.lock_state();
        if let Some(entry) = state.entries.get_mut(token)
            && entry.lease_id == Some(lease_id)
        {
            entry.lease_id = None;
        }
    }

    fn lock_state(&self) -> MutexGuard<'_, StoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
pub struct QueryCursorLease {
    store: Arc<QueryCursorStore>,
    token: String,
    lease_id: Uuid,
    data: QueryCursorData,
    completed: bool,
}

impl QueryCursorLease {
    #[must_use]
    pub fn data(&self) -> &QueryCursorData {
        &self.data
    }

    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn commit(
        mut self,
        next: Option<QueryCursorData>,
    ) -> Result<Option<String>, QueryCursorError> {
        let result = self
            .store
            .finish_lease(&self.token, self.lease_id, &self.data, next);
        if result.is_ok() {
            self.completed = true;
        }
        result
    }
}

impl Drop for QueryCursorLease {
    fn drop(&mut self) {
        if !self.completed {
            self.store.release_lease(&self.token, self.lease_id);
        }
    }
}

#[derive(Debug, Default)]
struct StoreState {
    entries: HashMap<String, StoredCursor>,
    order: VecDeque<String>,
}

impl StoreState {
    fn insert_new(&mut self, data: QueryCursorData, expires_at: Instant) -> String {
        let token = loop {
            let candidate = format!("{CURSOR_PREFIX}{}", Uuid::new_v4().simple());
            if !self.entries.contains_key(&candidate) {
                break candidate;
            }
        };
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

    fn remove(&mut self, token: &str) {
        self.entries.remove(token);
        self.order.retain(|candidate| candidate != token);
    }
}

#[derive(Debug)]
struct StoredCursor {
    data: QueryCursorData,
    expires_at: Instant,
    lease_id: Option<Uuid>,
}

fn validate_time_bound(value: Option<&str>) -> Result<(), QueryCursorError> {
    if value.is_some_and(|bound| bound.is_empty() || bound.chars().count() > MAX_TIME_BOUND_CHARS) {
        return Err(QueryCursorError::InvalidData(
            "time bound length is outside the server limit",
        ));
    }
    Ok(())
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

fn is_well_formed_token(token: &str) -> bool {
    token.len() == CURSOR_LENGTH
        && token.starts_with(CURSOR_PREFIX)
        && token[CURSOR_PREFIX.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
}

pub fn open_cursor_snapshot(
    root: &SafeRoot,
    snapshot: &CursorFileSnapshot,
) -> Result<SafeFile, QueryCursorFileError> {
    let safe_file = root.open_regular_file(&snapshot.relative_path)?;
    if safe_file.identity() != snapshot.file_identity {
        return Err(QueryCursorFileError::FileChanged);
    }
    if safe_file.size() < snapshot.file_size {
        return Err(QueryCursorFileError::FileTruncated);
    }
    Ok(safe_file)
}

#[derive(Debug, Error)]
pub enum QueryCursorError {
    #[error("query cursor capacity must be greater than zero")]
    InvalidCapacity,

    #[error("query cursor TTL must be greater than zero")]
    InvalidTtl,

    #[error("query cursor expiration cannot be represented")]
    ExpirationOverflow,

    #[error("invalid query cursor data: {0}")]
    InvalidData(&'static str),

    #[error("invalid query cursor continuation: {0}")]
    InvalidContinuation(&'static str),

    #[error("unknown or expired query cursor")]
    UnknownOrExpired,

    #[error("query cursor does not match the supplied search conditions")]
    QueryMismatch,

    #[error("query cursor is already being consumed")]
    Busy,

    #[error("query cursor lease was lost or expired")]
    LeaseLost,
}

#[derive(Debug, Error)]
pub enum QueryCursorFileError {
    #[error("cursor log file cannot be opened safely")]
    Open(#[from] SafeOpenError),

    #[error("cursor log file has been rotated or replaced")]
    FileChanged,

    #[error("cursor log file has been truncated")]
    FileTruncated,
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::Arc, thread};

    use tempfile::tempdir;

    use super::*;

    fn query() -> CursorQuery {
        CursorQuery {
            source_ids: vec!["payment-test".to_owned(), "order-test".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results: 50,
        }
    }

    fn snapshots() -> Vec<CursorFileSnapshot> {
        vec![
            CursorFileSnapshot {
                source_id: "payment-test".to_owned(),
                relative_path: PathBuf::from("application.log"),
                file_identity: FileIdentity {
                    device: 10,
                    inode: 20,
                },
                file_size: 100,
            },
            CursorFileSnapshot {
                source_id: "order-test".to_owned(),
                relative_path: PathBuf::from("application.log"),
                file_identity: FileIdentity {
                    device: 11,
                    inode: 21,
                },
                file_size: 200,
            },
        ]
    }

    fn cursor_data() -> QueryCursorData {
        QueryCursorData {
            query: query(),
            files: snapshots(),
            position: CursorPosition {
                file_index: 0,
                byte_offset: 20,
                line_number: 2,
            },
            cumulative_scanned_bytes: 20,
            cumulative_returned_results: 5,
        }
    }

    fn advanced_data() -> QueryCursorData {
        QueryCursorData {
            position: CursorPosition {
                file_index: 0,
                byte_offset: 60,
                line_number: 4,
            },
            cumulative_scanned_bytes: 60,
            cumulative_returned_results: 10,
            ..cursor_data()
        }
    }

    #[test]
    fn creates_opaque_cursor_and_binds_query() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = store.insert(cursor_data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("cursor should begin");

        assert!(token.starts_with(CURSOR_PREFIX));
        assert_eq!(token.len(), CURSOR_LENGTH);
        assert!(!token.contains("traceId"));
        assert!(!token.contains("application"));
        assert_eq!(lease.data(), &cursor_data());
    }

    #[test]
    fn rejects_changed_query_conditions() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = store.insert(cursor_data()).expect("cursor should be inserted");
        let mut changed = query();
        changed.keyword = "traceId=other".to_owned();

        assert!(matches!(
            store.begin(&token, &changed),
            Err(QueryCursorError::QueryMismatch)
        ));
    }

    #[test]
    fn prevents_concurrent_consumption_and_drop_releases_lease() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = store.insert(cursor_data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("first lease should begin");

        assert!(matches!(
            store.begin(&token, &query()),
            Err(QueryCursorError::Busy)
        ));
        drop(lease);
        assert!(store.begin(&token, &query()).is_ok());
    }

    #[test]
    fn commit_consumes_old_cursor_and_returns_fresh_cursor() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let old = store.insert(cursor_data()).expect("cursor should be inserted");
        let lease = store.begin(&old, &query()).expect("lease should begin");
        let next = lease
            .commit(Some(advanced_data()))
            .expect("continuation should commit")
            .expect("next cursor should be returned");

        assert_ne!(old, next);
        assert!(matches!(
            store.begin(&old, &query()),
            Err(QueryCursorError::UnknownOrExpired)
        ));
        let next_lease = store.begin(&next, &query()).expect("next cursor should begin");
        assert_eq!(next_lease.data().position.byte_offset, 60);
    }

    #[test]
    fn completing_last_page_consumes_cursor_without_replacement() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = store.insert(cursor_data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("lease should begin");

        assert_eq!(lease.commit(None).expect("cursor should complete"), None);
        assert!(store.is_empty());
    }

    #[test]
    fn invalid_continuation_keeps_original_cursor_available() {
        let store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = store.insert(cursor_data()).expect("cursor should be inserted");
        let lease = store.begin(&token, &query()).expect("lease should begin");
        let mut invalid = advanced_data();
        invalid.position.byte_offset = 10;

        assert!(matches!(
            lease.commit(Some(invalid)),
            Err(QueryCursorError::InvalidContinuation(_))
        ));
        assert!(store.begin(&token, &query()).is_ok());
    }

    #[test]
    fn expires_and_evicts_cursors() {
        let expiring = Arc::new(
            QueryCursorStore::new(10, Duration::from_millis(5))
                .expect("store should be created"),
        );
        let token = expiring
            .insert(cursor_data())
            .expect("cursor should be inserted");
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            expiring.begin(&token, &query()),
            Err(QueryCursorError::UnknownOrExpired)
        ));

        let bounded = Arc::new(
            QueryCursorStore::new(2, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let first = bounded
            .insert(cursor_data())
            .expect("first cursor should be inserted");
        let second = bounded
            .insert(cursor_data())
            .expect("second cursor should be inserted");
        let third = bounded
            .insert(cursor_data())
            .expect("third cursor should be inserted");
        assert!(matches!(
            bounded.begin(&first, &query()),
            Err(QueryCursorError::UnknownOrExpired)
        ));
        assert!(bounded.begin(&second, &query()).is_ok());
        assert!(bounded.begin(&third, &query()).is_ok());
    }

    #[test]
    fn new_store_invalidates_existing_cursor() {
        let first_store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("store should be created"),
        );
        let token = first_store
            .insert(cursor_data())
            .expect("cursor should be inserted");
        let new_store = Arc::new(
            QueryCursorStore::new(10, Duration::from_secs(60))
                .expect("new store should be created"),
        );

        assert!(matches!(
            new_store.begin(&token, &query()),
            Err(QueryCursorError::UnknownOrExpired)
        ));
    }

    #[test]
    fn rejects_invalid_cursor_data() {
        let store = QueryCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let invalid_path = QueryCursorData {
            files: vec![CursorFileSnapshot {
                relative_path: PathBuf::from("../secret.log"),
                ..snapshots()[0].clone()
            }],
            ..cursor_data()
        };
        let invalid_position = QueryCursorData {
            position: CursorPosition {
                file_index: 9,
                byte_offset: 0,
                line_number: 1,
            },
            ..cursor_data()
        };

        assert!(matches!(
            store.insert(invalid_path),
            Err(QueryCursorError::InvalidData(_))
        ));
        assert!(matches!(
            store.insert(invalid_position),
            Err(QueryCursorError::InvalidData(_))
        ));
    }

    #[test]
    fn file_snapshot_accepts_append_but_rejects_replace_and_truncate() {
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("application.log");
        fs::write(&path, "first page\n").expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let file = root
            .open_regular_file("application.log")
            .expect("fixture should open");
        let snapshot = CursorFileSnapshot {
            source_id: "payment-test".to_owned(),
            relative_path: PathBuf::from("application.log"),
            file_identity: file.identity(),
            file_size: file.size(),
        };

        fs::write(&path, "first page\nappended\n").expect("fixture should be appended");
        assert!(open_cursor_snapshot(&root, &snapshot).is_ok());

        fs::write(&path, "tiny\n").expect("fixture should be truncated");
        assert!(matches!(
            open_cursor_snapshot(&root, &snapshot),
            Err(QueryCursorFileError::FileTruncated)
        ));

        let rotated = directory.path().join("application.log.1");
        fs::rename(&path, rotated).expect("fixture should rotate");
        fs::write(&path, "replacement content\n").expect("replacement should be written");
        assert!(matches!(
            open_cursor_snapshot(&root, &snapshot),
            Err(QueryCursorFileError::FileChanged)
        ));
    }

    #[test]
    fn binds_cursor_to_request_defaults_and_values() {
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
        let query = CursorQuery::from_request(&request).expect("query should be created");

        assert_eq!(query, super::tests::query());
    }
}
