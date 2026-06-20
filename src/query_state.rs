use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet, VecDeque},
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    FileIdentity, MAX_SCAN_BYTES, MAX_SCAN_KEYWORD_CHARS, MAX_SCAN_RESULTS, QuerySummary,
    SourceFileSnapshot,
};

const CURSOR_PREFIX: &str = "cur_";
const MATCH_REFERENCE_PREFIX: &str = "mref_";
const TOKEN_HEX_LENGTH: usize = 32;
const MAX_SOURCE_ID_CHARS: usize = 128;
const MAX_FILE_ID_CHARS: usize = 256;
const MAX_RELATIVE_PATH_BYTES: usize = 4096;

pub const MAX_CURSOR_CANDIDATES: usize = 10_000;
pub const MAX_CURSOR_SOURCE_IDS: usize = 10;
pub const MAX_CURSOR_PAGES: u64 = 100;
pub const MAX_CURSOR_RESULTS_RETURNED: u64 = MAX_CURSOR_PAGES * MAX_SCAN_RESULTS as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryBinding {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<DateTime<FixedOffset>>,
    pub end_time: Option<DateTime<FixedOffset>>,
    pub max_results: usize,
}

impl QueryBinding {
    pub fn validate(&self) -> Result<(), QueryStateError> {
        if self.source_ids.is_empty() || self.source_ids.len() > MAX_CURSOR_SOURCE_IDS {
            return Err(QueryStateError::InvalidData(
                "source_ids count is outside the v1 limit",
            ));
        }

        let mut seen = HashSet::with_capacity(self.source_ids.len());
        for source_id in &self.source_ids {
            let chars = source_id.chars().count();
            if chars == 0
                || chars > MAX_SOURCE_ID_CHARS
                || source_id.contains('\0')
                || !seen.insert(source_id)
            {
                return Err(QueryStateError::InvalidData(
                    "source_ids must be bounded, non-empty and unique",
                ));
            }
        }

        let keyword_bytes = self.keyword.as_bytes();
        let keyword_chars = self.keyword.chars().count();
        if keyword_chars == 0
            || keyword_chars > MAX_SCAN_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
            || keyword_bytes.contains(&0)
        {
            return Err(QueryStateError::InvalidData(
                "keyword is outside the v1 literal search contract",
            ));
        }
        if self.max_results == 0 || self.max_results > MAX_SCAN_RESULTS {
            return Err(QueryStateError::InvalidData(
                "max_results is outside the v1 limit",
            ));
        }
        if self
            .start_time
            .as_ref()
            .zip(self.end_time.as_ref())
            .is_some_and(|(start, end)| start >= end)
        {
            return Err(QueryStateError::InvalidData(
                "start_time must be earlier than end_time",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultWatermark {
    pub timestamp_utc: Option<DateTime<Utc>>,
    pub source_index: usize,
    pub file_index: usize,
    pub line_number: u64,
    pub match_byte_offset: u64,
}

impl ResultWatermark {
    pub fn validate(&self) -> Result<(), QueryStateError> {
        if self.line_number == 0 {
            return Err(QueryStateError::InvalidData(
                "watermark line number must start at one",
            ));
        }
        Ok(())
    }
}

impl Ord for ResultWatermark {
    fn cmp(&self, other: &Self) -> Ordering {
        timestamp_cmp(&self.timestamp_utc, &other.timestamp_utc)
            .then_with(|| self.source_index.cmp(&other.source_index))
            .then_with(|| self.file_index.cmp(&other.file_index))
            .then_with(|| self.line_number.cmp(&other.line_number))
            .then_with(|| self.match_byte_offset.cmp(&other.match_byte_offset))
    }
}

impl PartialOrd for ResultWatermark {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn timestamp_cmp(left: &Option<DateTime<Utc>>, right: &Option<DateTime<Utc>>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorCandidate {
    pub source_index: usize,
    pub file_index: usize,
    pub snapshot: SourceFileSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CumulativeQueryUsage {
    pub pages_returned: u64,
    pub files_scanned: u64,
    pub bytes_scanned: u64,
    pub lines_scanned: u64,
    pub raw_matches: u64,
    pub eligible_matches: u64,
    pub results_returned: u64,
    pub returned_content_bytes: u64,
}

impl CumulativeQueryUsage {
    pub fn add_page(&mut self, summary: &QuerySummary) -> Result<(), QueryStateError> {
        self.pages_returned = checked_add(self.pages_returned, 1)?;
        self.files_scanned = checked_add(
            self.files_scanned,
            u64::try_from(summary.files_scanned).map_err(|_| QueryStateError::CounterOverflow)?,
        )?;
        self.bytes_scanned = checked_add(self.bytes_scanned, summary.bytes_scanned)?;
        self.lines_scanned = checked_add(self.lines_scanned, summary.lines_scanned)?;
        self.raw_matches = checked_add(self.raw_matches, summary.raw_matches)?;
        self.eligible_matches = checked_add(self.eligible_matches, summary.eligible_matches)?;
        self.results_returned = checked_add(
            self.results_returned,
            u64::try_from(summary.returned_results)
                .map_err(|_| QueryStateError::CounterOverflow)?,
        )?;
        self.returned_content_bytes = checked_add(
            self.returned_content_bytes,
            u64::try_from(summary.returned_content_bytes)
                .map_err(|_| QueryStateError::CounterOverflow)?,
        )?;
        self.validate()
    }

    pub fn validate(&self) -> Result<(), QueryStateError> {
        if self.pages_returned > MAX_CURSOR_PAGES
            || self.bytes_scanned > MAX_SCAN_BYTES
            || self.results_returned > MAX_CURSOR_RESULTS_RETURNED
            || self.returned_content_bytes > MAX_SCAN_BYTES
        {
            return Err(QueryStateError::CumulativeLimit);
        }
        Ok(())
    }
}

fn checked_add(left: u64, right: u64) -> Result<u64, QueryStateError> {
    left.checked_add(right)
        .ok_or(QueryStateError::CounterOverflow)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCursorData {
    pub query: QueryBinding,
    pub candidates: Vec<CursorCandidate>,
    pub after: ResultWatermark,
    pub usage: CumulativeQueryUsage,
}

impl SearchCursorData {
    pub fn validate(&self) -> Result<(), QueryStateError> {
        self.query.validate()?;
        self.after.validate()?;
        self.usage.validate()?;
        if self.usage.pages_returned == 0 || self.usage.results_returned == 0 {
            return Err(QueryStateError::InvalidData(
                "cursor usage must describe at least one returned page and result",
            ));
        }
        if self.candidates.is_empty() || self.candidates.len() > MAX_CURSOR_CANDIDATES {
            return Err(QueryStateError::InvalidData(
                "candidate snapshot count is outside the service limit",
            ));
        }

        let mut seen = HashSet::with_capacity(self.candidates.len());
        let mut watermark_candidate_exists = false;
        for candidate in &self.candidates {
            if candidate.source_index >= self.query.source_ids.len()
                || candidate.snapshot.source_id()
                    != self.query.source_ids[candidate.source_index].as_str()
            {
                return Err(QueryStateError::InvalidData(
                    "candidate snapshot does not belong to the bound query",
                ));
            }
            if !seen.insert((
                candidate.source_index,
                candidate.file_index,
                candidate.snapshot.file_id().to_owned(),
            )) {
                return Err(QueryStateError::InvalidData(
                    "candidate snapshot contains duplicate entries",
                ));
            }
            if candidate.source_index == self.after.source_index
                && candidate.file_index == self.after.file_index
            {
                watermark_candidate_exists = true;
            }
        }
        if !watermark_candidate_exists {
            return Err(QueryStateError::InvalidData(
                "watermark does not identify a candidate snapshot",
            ));
        }
        Ok(())
    }

    fn validate_continuation(&self, next: &Self) -> Result<(), QueryStateError> {
        next.validate()
            .map_err(|_| QueryStateError::InvalidContinuation("next cursor state is invalid"))?;
        if next.query != self.query {
            return Err(QueryStateError::InvalidContinuation(
                "query binding changed between pages",
            ));
        }
        if next.candidates != self.candidates {
            return Err(QueryStateError::InvalidContinuation(
                "candidate snapshot changed between pages",
            ));
        }
        if next.after <= self.after {
            return Err(QueryStateError::InvalidContinuation(
                "result watermark must move forward",
            ));
        }
        if next.usage.pages_returned != self.usage.pages_returned.saturating_add(1)
            || next.usage.files_scanned < self.usage.files_scanned
            || next.usage.bytes_scanned < self.usage.bytes_scanned
            || next.usage.lines_scanned < self.usage.lines_scanned
            || next.usage.raw_matches < self.usage.raw_matches
            || next.usage.eligible_matches < self.usage.eligible_matches
            || next.usage.results_returned < self.usage.results_returned
            || next.usage.returned_content_bytes < self.usage.returned_content_bytes
        {
            return Err(QueryStateError::InvalidContinuation(
                "cumulative usage must advance monotonically by one page",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReferenceData {
    pub source_id: String,
    pub file_id: String,
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
    pub fn validate(&self) -> Result<(), QueryStateError> {
        if self.source_id.is_empty()
            || self.source_id.chars().count() > MAX_SOURCE_ID_CHARS
            || self.file_id.is_empty()
            || self.file_id.chars().count() > MAX_FILE_ID_CHARS
        {
            return Err(QueryStateError::InvalidData(
                "match reference source or file identifier is invalid",
            ));
        }
        validate_relative_path(&self.relative_path)?;

        let keyword_bytes = self.keyword.as_bytes();
        let keyword_chars = self.keyword.chars().count();
        if keyword_chars == 0
            || keyword_chars > MAX_SCAN_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
            || keyword_bytes.contains(&0)
        {
            return Err(QueryStateError::InvalidData(
                "match reference keyword is invalid",
            ));
        }
        if self.line_number == 0 || self.line_start_offset > self.match_byte_offset {
            return Err(QueryStateError::InvalidData(
                "match reference line or byte offsets are invalid",
            ));
        }
        let keyword_len =
            u64::try_from(keyword_bytes.len()).map_err(|_| QueryStateError::CounterOverflow)?;
        let match_end = self
            .match_byte_offset
            .checked_add(keyword_len)
            .ok_or(QueryStateError::CounterOverflow)?;
        if match_end > self.file_size_at_match {
            return Err(QueryStateError::InvalidData(
                "match reference byte range exceeds the file snapshot",
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

fn validate_relative_path(path: &Path) -> Result<(), QueryStateError> {
    if path.as_os_str().as_encoded_bytes().len() > MAX_RELATIVE_PATH_BYTES {
        return Err(QueryStateError::InvalidData(
            "relative path is outside the service limit",
        ));
    }
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(QueryStateError::InvalidData(
                    "relative path must be normalized below its source root",
                ));
            }
        }
    }
    if !has_component {
        return Err(QueryStateError::InvalidData(
            "relative path must identify a file",
        ));
    }
    Ok(())
}

#[derive(Debug)]
pub struct SearchCursorStore {
    capacity: usize,
    ttl: Duration,
    state: Mutex<CursorStoreState>,
}

impl SearchCursorStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, QueryStateError> {
        validate_store_config(capacity, ttl)?;
        Ok(Self {
            capacity,
            ttl,
            state: Mutex::new(CursorStoreState::default()),
        })
    }

    pub fn insert(&self, data: SearchCursorData) -> Result<String, QueryStateError> {
        data.validate()?;
        let now = Instant::now();
        let expires_at = expiration(now, self.ttl)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            if !state.evict_oldest_unleased() {
                return Err(QueryStateError::CapacityBusy);
            }
        }
        Ok(state.insert_new(data, expires_at))
    }

    pub fn begin(
        self: &Arc<Self>,
        token: &str,
        expected_query: &QueryBinding,
    ) -> Result<SearchCursorLease, QueryStateError> {
        expected_query.validate()?;
        validate_token(token, CURSOR_PREFIX)?;
        let now = Instant::now();
        let lease_id = Uuid::new_v4();
        let data = {
            let mut state = self.lock_state();
            state.purge_expired(now);
            let entry = state
                .entries
                .get_mut(token)
                .ok_or(QueryStateError::UnknownOrExpired)?;
            if &entry.data.query != expected_query {
                return Err(QueryStateError::QueryMismatch);
            }
            if entry.lease_id.is_some() {
                return Err(QueryStateError::Busy);
            }
            entry.lease_id = Some(lease_id);
            entry.data.clone()
        };
        Ok(SearchCursorLease {
            store: Arc::clone(self),
            token: token.to_owned(),
            lease_id,
            data,
            completed: false,
        })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn finish_lease(
        &self,
        token: &str,
        lease_id: Uuid,
        current: &SearchCursorData,
        next: Option<SearchCursorData>,
    ) -> Result<Option<String>, QueryStateError> {
        if let Some(next_data) = &next {
            current.validate_continuation(next_data)?;
        }
        let now = Instant::now();
        let next_expiration = next
            .as_ref()
            .map(|_| expiration(now, self.ttl))
            .transpose()?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        let entry = state.entries.get(token).ok_or(QueryStateError::LeaseLost)?;
        if entry.lease_id != Some(lease_id) || &entry.data != current {
            return Err(QueryStateError::LeaseLost);
        }
        state.remove(token);
        match (next, next_expiration) {
            (Some(data), Some(expires_at)) => Ok(Some(state.insert_new(data, expires_at))),
            (None, None) => Ok(None),
            _ => Err(QueryStateError::LeaseLost),
        }
    }

    fn release_lease(&self, token: &str, lease_id: Uuid) {
        let now = Instant::now();
        let mut state = self.lock_state();
        let should_remove = if let Some(entry) = state.entries.get_mut(token) {
            if entry.lease_id == Some(lease_id) {
                entry.lease_id = None;
                entry.expires_at <= now
            } else {
                false
            }
        } else {
            false
        };
        if should_remove {
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
    pub const fn data(&self) -> &SearchCursorData {
        &self.data
    }

    pub fn commit(
        mut self,
        next: Option<SearchCursorData>,
    ) -> Result<Option<String>, QueryStateError> {
        let result = self
            .store
            .finish_lease(&self.token, self.lease_id, &self.data, next)?;
        self.completed = true;
        Ok(result)
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
        let token = unique_token(CURSOR_PREFIX, &self.entries);
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

    fn purge_expired(&mut self, now: Instant) {
        let expired: HashSet<String> = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.lease_id.is_none() && entry.expires_at <= now)
            .map(|(token, _)| token.clone())
            .collect();
        for token in expired {
            self.entries.remove(&token);
        }
        self.order.retain(|token| self.entries.contains_key(token));
    }

    fn evict_oldest_unleased(&mut self) -> bool {
        let count = self.order.len();
        for _ in 0..count {
            let Some(token) = self.order.pop_front() else {
                return false;
            };
            match self.entries.get(&token) {
                Some(entry) if entry.lease_id.is_some() => self.order.push_back(token),
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

#[derive(Debug)]
pub struct MatchReferenceStore {
    capacity: usize,
    ttl: Duration,
    state: Mutex<MatchStoreState>,
}

impl MatchReferenceStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, QueryStateError> {
        validate_store_config(capacity, ttl)?;
        Ok(Self {
            capacity,
            ttl,
            state: Mutex::new(MatchStoreState::default()),
        })
    }

    pub fn insert(&self, data: MatchReferenceData) -> Result<String, QueryStateError> {
        data.validate()?;
        let now = Instant::now();
        let expires_at = expiration(now, self.ttl)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }
        let token = unique_token(MATCH_REFERENCE_PREFIX, &state.entries);
        state.order.push_back(token.clone());
        state
            .entries
            .insert(token.clone(), StoredMatch { data, expires_at });
        Ok(token)
    }

    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {
        validate_token(token, MATCH_REFERENCE_PREFIX)?;
        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state
            .entries
            .get(token)
            .map(|entry| entry.data.clone())
            .ok_or(QueryStateError::UnknownOrExpired)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock_state(&self) -> MutexGuard<'_, MatchStoreState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug, Default)]
struct MatchStoreState {
    entries: HashMap<String, StoredMatch>,
    order: VecDeque<String>,
}

impl MatchStoreState {
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
struct StoredMatch {
    data: MatchReferenceData,
    expires_at: Instant,
}

fn validate_store_config(capacity: usize, ttl: Duration) -> Result<(), QueryStateError> {
    if capacity == 0 {
        return Err(QueryStateError::InvalidCapacity);
    }
    if ttl == Duration::ZERO {
        return Err(QueryStateError::InvalidTtl);
    }
    Ok(())
}

fn expiration(now: Instant, ttl: Duration) -> Result<Instant, QueryStateError> {
    now.checked_add(ttl)
        .ok_or(QueryStateError::ExpirationOverflow)
}

fn validate_token(token: &str, prefix: &str) -> Result<(), QueryStateError> {
    if token.len() != prefix.len() + TOKEN_HEX_LENGTH
        || !token.starts_with(prefix)
        || !token[prefix.len()..]
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(QueryStateError::UnknownOrExpired);
    }
    Ok(())
}

fn unique_token<T>(prefix: &str, entries: &HashMap<String, T>) -> String {
    loop {
        let candidate = format!("{prefix}{}", Uuid::new_v4().simple());
        if !entries.contains_key(&candidate) {
            return candidate;
        }
    }
}

#[derive(Debug, Error)]
pub enum QueryStateError {
    #[error("query state capacity must be greater than zero")]
    InvalidCapacity,

    #[error("query state TTL must be greater than zero")]
    InvalidTtl,

    #[error("query state expiration cannot be represented")]
    ExpirationOverflow,

    #[error("invalid query state: {0}")]
    InvalidData(&'static str),

    #[error("query state resource counter overflowed")]
    CounterOverflow,

    #[error("query state cumulative resource limit was reached")]
    CumulativeLimit,

    #[error("query cursor does not match the supplied query")]
    QueryMismatch,

    #[error("query cursor is already in use")]
    Busy,

    #[error("all query cursor slots are currently leased")]
    CapacityBusy,

    #[error("query cursor lease was lost")]
    LeaseLost,

    #[error("invalid query cursor continuation: {0}")]
    InvalidContinuation(&'static str),

    #[error("unknown or expired query state token")]
    UnknownOrExpired,
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf, thread};

    use tempfile::tempdir;

    use crate::{
        AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, SourceRegistry,
    };

    use super::*;

    fn binding() -> QueryBinding {
        QueryBinding {
            source_ids: vec!["payment-test".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            max_results: 2,
        }
    }

    fn snapshot() -> SourceFileSnapshot {
        let root = tempdir().expect("temporary source root should be created");
        fs::write(root.path().join("application.log"), "traceId=abc123\n")
            .expect("fixture should be written");
        let registry = SourceRegistry::from_config(AppConfig {
            version: CONFIG_VERSION,
            sources: vec![LogSourceConfig {
                source_id: "payment-test".to_owned(),
                name: "Payment".to_owned(),
                description: String::new(),
                service: "payment".to_owned(),
                environment: "test".to_owned(),
                tags: Vec::new(),
                enabled: true,
                encoding: Encoding::Utf8,
                root: root.path().to_path_buf(),
                files: vec![PathBuf::from("application.log")],
                directories: Vec::new(),
                timestamp_rule: None,
            }],
            limits: LimitsConfig::default(),
        })
        .expect("registry should build");
        registry
            .get("payment-test")
            .expect("source should exist")
            .snapshot_files(1)
            .expect("snapshot should succeed")
            .remove(0)
    }

    fn usage() -> CumulativeQueryUsage {
        CumulativeQueryUsage {
            pages_returned: 1,
            files_scanned: 1,
            bytes_scanned: 16,
            lines_scanned: 1,
            raw_matches: 1,
            eligible_matches: 1,
            results_returned: 1,
            returned_content_bytes: 16,
        }
    }

    fn cursor_data() -> SearchCursorData {
        SearchCursorData {
            query: binding(),
            candidates: vec![CursorCandidate {
                source_index: 0,
                file_index: 0,
                snapshot: snapshot(),
            }],
            after: ResultWatermark {
                timestamp_utc: None,
                source_index: 0,
                file_index: 0,
                line_number: 1,
                match_byte_offset: 0,
            },
            usage: usage(),
        }
    }

    #[test]
    fn cursor_lease_releases_on_drop_and_commits_new_token() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store
            .insert(cursor_data())
            .expect("cursor should be inserted");

        {
            let lease = store
                .begin(&token, &binding())
                .expect("cursor should lease");
            assert!(matches!(
                store.begin(&token, &binding()),
                Err(QueryStateError::Busy)
            ));
            drop(lease);
        }

        let lease = store
            .begin(&token, &binding())
            .expect("released cursor should lease again");
        let mut next = lease.data().clone();
        next.after.line_number = 2;
        next.after.match_byte_offset = 20;
        next.usage.pages_returned = 2;
        next.usage.files_scanned = 2;
        next.usage.bytes_scanned = 32;
        next.usage.lines_scanned = 2;
        next.usage.raw_matches = 2;
        next.usage.eligible_matches = 2;
        next.usage.results_returned = 2;
        next.usage.returned_content_bytes = 32;
        let next_token = lease
            .commit(Some(next))
            .expect("continuation should commit")
            .expect("next token should exist");

        assert_ne!(token, next_token);
        assert!(matches!(
            store.begin(&token, &binding()),
            Err(QueryStateError::UnknownOrExpired)
        ));
        assert!(store.begin(&next_token, &binding()).is_ok());
    }

    #[test]
    fn query_mismatch_does_not_consume_cursor() {
        let store = Arc::new(
            SearchCursorStore::new(10, Duration::from_secs(60)).expect("store should be created"),
        );
        let token = store
            .insert(cursor_data())
            .expect("cursor should be inserted");
        let mut changed = binding();
        changed.keyword = "other".to_owned();

        assert!(matches!(
            store.begin(&token, &changed),
            Err(QueryStateError::QueryMismatch)
        ));
        assert!(store.begin(&token, &binding()).is_ok());
    }

    #[test]
    fn cursor_expires_and_capacity_evicts_oldest_unleased() {
        let expiring = Arc::new(
            SearchCursorStore::new(2, Duration::from_millis(5)).expect("store should be created"),
        );
        let token = expiring
            .insert(cursor_data())
            .expect("cursor should be inserted");
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            expiring.begin(&token, &binding()),
            Err(QueryStateError::UnknownOrExpired)
        ));

        let bounded = Arc::new(
            SearchCursorStore::new(1, Duration::from_secs(60)).expect("store should be created"),
        );
        let first = bounded
            .insert(cursor_data())
            .expect("first cursor should insert");
        let second = bounded
            .insert(cursor_data())
            .expect("second cursor should insert");
        assert!(matches!(
            bounded.begin(&first, &binding()),
            Err(QueryStateError::UnknownOrExpired)
        ));
        assert!(bounded.begin(&second, &binding()).is_ok());
    }

    #[test]
    fn match_references_are_opaque_bounded_and_expire() {
        let store =
            MatchReferenceStore::new(1, Duration::from_millis(5)).expect("store should be created");
        let snapshot = snapshot();
        let data = MatchReferenceData {
            source_id: snapshot.source_id().to_owned(),
            file_id: snapshot.file_id().to_owned(),
            relative_path: snapshot.relative_path().to_path_buf(),
            file_identity: snapshot.identity(),
            file_size_at_match: snapshot.size_at_snapshot(),
            line_number: 1,
            line_start_offset: 0,
            match_byte_offset: 0,
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
        };
        let first = store
            .insert(data.clone())
            .expect("first reference should insert");
        let second = store
            .insert(data.clone())
            .expect("second reference should insert");

        assert!(first.starts_with(MATCH_REFERENCE_PREFIX));
        assert!(!first.contains("payment"));
        assert!(matches!(
            store.resolve(&first),
            Err(QueryStateError::UnknownOrExpired)
        ));
        assert_eq!(store.resolve(&second).expect("second should resolve"), data);
        thread::sleep(Duration::from_millis(20));
        assert!(matches!(
            store.resolve(&second),
            Err(QueryStateError::UnknownOrExpired)
        ));
    }
}
