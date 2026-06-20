use std::{
    collections::{HashMap, VecDeque},
    sync::{Mutex, MutexGuard},
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset, Utc};
use thiserror::Error;
use uuid::Uuid;

use crate::{
    MAX_SCAN_KEYWORD_CHARS, QueryMatch, QuerySummary, SourceFileSnapshot,
};

const CURSOR_PREFIX: &str = "cur_";
const MATCH_REFERENCE_PREFIX: &str = "mref_";
const TOKEN_HEX_LENGTH: usize = 32;
pub const MAX_CURSOR_CANDIDATES: usize = 10_000;
pub const MAX_CURSOR_SOURCE_IDS: usize = 10;

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
        let mut seen = std::collections::HashSet::with_capacity(self.source_ids.len());
        if self
            .source_ids
            .iter()
            .any(|source_id| source_id.is_empty() || !seen.insert(source_id))
        {
            return Err(QueryStateError::InvalidData(
                "source_ids must be non-empty and unique",
            ));
        }
        let keyword_bytes = self.keyword.as_bytes();
        if self.keyword.chars().count() == 0
            || self.keyword.chars().count() > MAX_SCAN_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(QueryStateError::InvalidData(
                "keyword is outside the v1 literal search contract",
            ));
        }
        if self.max_results == 0 || self.max_results > crate::MAX_SCAN_RESULTS {
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
        self.pages_returned = self
            .pages_returned
            .checked_add(1)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.files_scanned = self
            .files_scanned
            .checked_add(u64::try_from(summary.files_scanned).map_err(|_| QueryStateError::CounterOverflow)?)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.bytes_scanned = self
            .bytes_scanned
            .checked_add(summary.bytes_scanned)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.lines_scanned = self
            .lines_scanned
            .checked_add(summary.lines_scanned)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.raw_matches = self
            .raw_matches
            .checked_add(summary.raw_matches)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.eligible_matches = self
            .eligible_matches
            .checked_add(summary.eligible_matches)
            .ok_or(QueryStateError::CounterOverflow)?;
        self.results_returned = self
            .results_returned
            .checked_add(
                u64::try_from(summary.returned_results)
                    .map_err(|_| QueryStateError::CounterOverflow)?,
            )
            .ok_or(QueryStateError::CounterOverflow)?;
        self.returned_content_bytes = self
            .returned_content_bytes
            .checked_add(
                u64::try_from(summary.returned_content_bytes)
                    .map_err(|_| QueryStateError::CounterOverflow)?,
            )
            .ok_or(QueryStateError::CounterOverflow)?;
        Ok(())
    }
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
        if self.candidates.is_empty() || self.candidates.len() > MAX_CURSOR_CANDIDATES {
            return Err(QueryStateError::InvalidData(
                "candidate snapshot count is outside the service limit",
            ));
        }
        for candidate in &self.candidates {
            if candidate.source_index >= self.query.source_ids.len()
                || candidate.snapshot.source_id()
                    != self.query.source_ids[candidate.source_index].as_str()
            {
                return Err(QueryStateError::InvalidData(
                    "candidate snapshot does not belong to the bound query",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReferenceData {
    pub source_id: String,
    pub file_id: String,
    pub relative_path: std::path::PathBuf,
    pub file_identity: crate::FileIdentity,
    pub file_size_at_match: u64,
    pub line_number: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
    pub keyword: String,
    pub case_sensitive: bool,
}

impl MatchReferenceData {
    pub fn from_query_match(
        query: &QueryBinding,
        result: &QueryMatch,
    ) -> Result<Self, QueryStateError> {
        let location = result.location();
        let data = Self {
            source_id: result.source_id.clone(),
            file_id: result.file_id.clone(),
            relative_path: location.snapshot.relative_path().to_path_buf(),
            file_identity: location.snapshot.identity(),
            file_size_at_match: location.snapshot.size_at_snapshot(),
            line_number: result.line_number,
            line_start_offset: result.line_start_offset,
            match_byte_offset: result.match_byte_offset,
            keyword: query.keyword.clone(),
            case_sensitive: query.case_sensitive,
        };
        data.validate()?;
        Ok(data)
    }

    pub fn validate(&self) -> Result<(), QueryStateError> {
        if self.source_id.is_empty() || self.file_id.is_empty() {
            return Err(QueryStateError::InvalidData(
                "match reference source and file identifiers must be non-empty",
            ));
        }
        if self.line_number == 0 || self.line_start_offset > self.match_byte_offset {
            return Err(QueryStateError::InvalidData(
                "match reference line or byte offsets are invalid",
            ));
        }
        let keyword_len = u64::try_from(self.keyword.len()).map_err(|_| QueryStateError::CounterOverflow)?;
        let match_end = self
            .match_byte_offset
            .checked_add(keyword_len)
            .ok_or(QueryStateError::CounterOverflow)?;
        if keyword_len == 0 || match_end > self.file_size_at_match {
            return Err(QueryStateError::InvalidData(
                "match reference byte range exceeds the file snapshot",
            ));
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct SearchCursorStore {
    inner: TokenStore<SearchCursorData>,
}

impl SearchCursorStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, QueryStateError> {
        Ok(Self {
            inner: TokenStore::new(CURSOR_PREFIX, capacity, ttl)?,
        })
    }

    pub fn insert(&self, data: SearchCursorData) -> Result<String, QueryStateError> {
        data.validate()?;
        self.inner.insert(data)
    }

    pub fn take(
        &self,
        token: &str,
        expected_query: &QueryBinding,
    ) -> Result<SearchCursorData, QueryStateError> {
        expected_query.validate()?;
        let data = self.inner.take(token)?;
        if &data.query != expected_query {
            self.inner.insert_with_token(token, data)?;
            return Err(QueryStateError::QueryMismatch);
        }
        Ok(data)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[derive(Debug)]
pub struct MatchReferenceStore {
    inner: TokenStore<MatchReferenceData>,
}

impl MatchReferenceStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, QueryStateError> {
        Ok(Self {
            inner: TokenStore::new(MATCH_REFERENCE_PREFIX, capacity, ttl)?,
        })
    }

    pub fn insert(&self, data: MatchReferenceData) -> Result<String, QueryStateError> {
        data.validate()?;
        self.inner.insert(data)
    }

    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, QueryStateError> {
        self.inner.get(token)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[derive(Debug)]
struct TokenStore<T> {
    prefix: &'static str,
    capacity: usize,
    ttl: Duration,
    state: Mutex<TokenStoreState<T>>,
}

impl<T: Clone> TokenStore<T> {
    fn new(prefix: &'static str, capacity: usize, ttl: Duration) -> Result<Self, QueryStateError> {
        if capacity == 0 {
            return Err(QueryStateError::InvalidCapacity);
        }
        if ttl == Duration::ZERO {
            return Err(QueryStateError::InvalidTtl);
        }
        Ok(Self {
            prefix,
            capacity,
            ttl,
            state: Mutex::new(TokenStoreState::default()),
        })
    }

    fn insert(&self, data: T) -> Result<String, QueryStateError> {
        let token = format!("{}{}", self.prefix, Uuid::new_v4().simple());
        self.insert_with_token(&token, data)?;
        Ok(token)
    }

    fn insert_with_token(&self, token: &str, data: T) -> Result<(), QueryStateError> {
        let now = Instant::now();
        let expires_at = now
            .checked_add(self.ttl)
            .ok_or(QueryStateError::ExpirationOverflow)?;
        let mut state = self.lock_state();
        state.purge_expired(now);
        while state.entries.len() >= self.capacity {
            state.evict_oldest();
        }
        state.order.retain(|existing| existing != token);
        state.order.push_back(token.to_owned());
        state
            .entries
            .insert(token.to_owned(), StoredToken { data, expires_at });
        Ok(())
    }

    fn get(&self, token: &str) -> Result<T, QueryStateError> {
        self.validate_token(token)?;
        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state
            .entries
            .get(token)
            .map(|stored| stored.data.clone())
            .ok_or(QueryStateError::UnknownOrExpired)
    }

    fn take(&self, token: &str) -> Result<T, QueryStateError> {
        self.validate_token(token)?;
        let now = Instant::now();
        let mut state = self.lock_state();
        state.purge_expired(now);
        state
            .entries
            .remove(token)
            .map(|stored| stored.data)
            .ok_or(QueryStateError::UnknownOrExpired)
    }

    fn len(&self) -> usize {
        let mut state = self.lock_state();
        state.purge_expired(Instant::now());
        state.entries.len()
    }

    fn validate_token(&self, token: &str) -> Result<(), QueryStateError> {
        if token.len() != self.prefix.len() + TOKEN_HEX_LENGTH
            || !token.starts_with(self.prefix)
            || !token[self.prefix.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(QueryStateError::UnknownOrExpired);
        }
        Ok(())
    }

    fn lock_state(&self) -> MutexGuard<'_, TokenStoreState<T>> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[derive(Debug)]
struct StoredToken<T> {
    data: T,
    expires_at: Instant,
}

#[derive(Debug)]
struct TokenStoreState<T> {
    entries: HashMap<String, StoredToken<T>>,
    order: VecDeque<String>,
}

impl<T> Default for TokenStoreState<T> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<T> TokenStoreState<T> {
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

    #[error("query cursor does not match the supplied query")]
    QueryMismatch,

    #[error("unknown or expired query state token")]
    UnknownOrExpired,
}
