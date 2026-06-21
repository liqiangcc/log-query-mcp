use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::{
    QueryMatch, QueryRequest, SourceFileSnapshot,
    state_store::{ExpiringStore, StateStoreError},
};

const SEARCH_CURSOR_PREFIX: &str = "cursor_";
pub const MAX_CURSOR_CANDIDATES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorQueryBinding {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub max_results: usize,
}

impl CursorQueryBinding {
    pub fn from_request(request: &QueryRequest, max_results: usize) -> Result<Self, SearchCursorError> {
        let value = Self {
            source_ids: request.source_ids.clone(),
            keyword: request.keyword.clone(),
            case_sensitive: request.case_sensitive,
            start_time: request.start_time.clone(),
            end_time: request.end_time.clone(),
            max_results,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), SearchCursorError> {
        if self.source_ids.is_empty() || self.source_ids.len() > 10 {
            return Err(SearchCursorError::InvalidData(
                "cursor source count is outside the v1 limit",
            ));
        }
        if self.source_ids.iter().any(|value| value.is_empty() || value.chars().count() > 128) {
            return Err(SearchCursorError::InvalidData("cursor source ID is invalid"));
        }
        let mut sorted = self.source_ids.clone();
        sorted.sort();
        sorted.dedup();
        if sorted.len() != self.source_ids.len() {
            return Err(SearchCursorError::InvalidData("cursor sources must be unique"));
        }
        if self.keyword.is_empty()
            || self.keyword.chars().count() > 256
            || self.keyword.as_bytes().contains(&b'\n')
            || self.keyword.as_bytes().contains(&b'\r')
        {
            return Err(SearchCursorError::InvalidData("cursor keyword is invalid"));
        }
        if self.max_results == 0 || self.max_results > 200 {
            return Err(SearchCursorError::InvalidData(
                "cursor page size is outside the v1 limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorCandidate {
    pub source_order: usize,
    pub file_order: usize,
    pub snapshot: SourceFileSnapshot,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct QueryWatermark {
    timestamp: Option<DateTime<Utc>>,
    source_order: usize,
    file_order: usize,
    line_number: u64,
    match_byte_offset: u64,
}

impl QueryWatermark {
    #[must_use]
    pub fn from_match(query_match: &QueryMatch, source_order: usize, file_order: usize) -> Self {
        Self {
            timestamp: query_match.timestamp.as_ref().map(|value| value.with_timezone(&Utc)),
            source_order,
            file_order,
            line_number: query_match.line_number,
            match_byte_offset: query_match.match_byte_offset,
        }
    }

    #[must_use]
    pub const fn source_order(&self) -> usize {
        self.source_order
    }

    #[must_use]
    pub const fn file_order(&self) -> usize {
        self.file_order
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchCursorData {
    pub query: CursorQueryBinding,
    pub candidates: Vec<CursorCandidate>,
    pub watermark: QueryWatermark,
    pub cumulative_scan_bytes: u64,
    pub cumulative_files_scanned: u64,
    pub cumulative_results_returned: u64,
}

impl SearchCursorData {
    pub fn validate(&self) -> Result<(), SearchCursorError> {
        self.query.validate()?;
        if self.candidates.is_empty() || self.candidates.len() > MAX_CURSOR_CANDIDATES {
            return Err(SearchCursorError::InvalidData(
                "cursor candidate count is outside the v1 limit",
            ));
        }
        for candidate in &self.candidates {
            let expected_source = self
                .query
                .source_ids
                .get(candidate.source_order)
                .ok_or(SearchCursorError::InvalidData(
                    "cursor candidate source order is invalid",
                ))?;
            if candidate.snapshot.source_id() != expected_source {
                return Err(SearchCursorError::InvalidData(
                    "cursor candidate does not belong to its bound source",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct SearchCursorStore {
    inner: ExpiringStore<SearchCursorData>,
}

impl SearchCursorStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, SearchCursorError> {
        Ok(Self {
            inner: ExpiringStore::new(SEARCH_CURSOR_PREFIX, capacity, ttl)?,
        })
    }

    pub fn insert(&self, value: SearchCursorData) -> Result<String, SearchCursorError> {
        value.validate()?;
        Ok(self.inner.insert(value)?)
    }

    pub fn take_for_query(
        &self,
        token: &str,
        expected_query: &CursorQueryBinding,
    ) -> Result<SearchCursorData, SearchCursorError> {
        let value = self.inner.get_cloned(token)?;
        if &value.query != expected_query {
            return Err(SearchCursorError::QueryMismatch);
        }
        self.inner.take(token).map_err(Into::into)
    }

    pub fn replace(
        &self,
        old_token: &str,
        expected_query: &CursorQueryBinding,
        next: SearchCursorData,
    ) -> Result<String, SearchCursorError> {
        let previous = self.take_for_query(old_token, expected_query)?;
        if previous.query != next.query {
            return Err(SearchCursorError::QueryMismatch);
        }
        self.insert(next)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }
}

#[derive(Debug, Error)]
pub enum SearchCursorError {
    #[error("invalid search cursor data: {0}")]
    InvalidData(&'static str),

    #[error("unknown or expired search cursor")]
    UnknownOrExpired,

    #[error("search cursor does not belong to these query parameters")]
    QueryMismatch,

    #[error("search cursor store configuration is invalid")]
    InvalidStore,
}

impl From<StateStoreError> for SearchCursorError {
    fn from(error: StateStoreError) -> Self {
        match error {
            StateStoreError::UnknownOrExpired => Self::UnknownOrExpired,
            StateStoreError::InvalidConfiguration | StateStoreError::ExpirationOverflow => {
                Self::InvalidStore
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::FileIdentity;

    use super::*;

    fn binding() -> CursorQueryBinding {
        CursorQueryBinding {
            source_ids: vec!["payment-test".to_owned()],
            keyword: "abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            max_results: 50,
        }
    }

    fn cursor() -> SearchCursorData {
        SearchCursorData {
            query: binding(),
            candidates: vec![CursorCandidate {
                source_order: 0,
                file_order: 0,
                snapshot: SourceFileSnapshot::from_parts_for_state(
                    "payment-test".to_owned(),
                    "file_test".to_owned(),
                    PathBuf::from("application.log"),
                    FileIdentity { device: 1, inode: 2 },
                    4096,
                ),
            }],
            watermark: QueryWatermark {
                timestamp: None,
                source_order: 0,
                file_order: 0,
                line_number: 42,
                match_byte_offset: 100,
            },
            cumulative_scan_bytes: 4096,
            cumulative_files_scanned: 1,
            cumulative_results_returned: 50,
        }
    }

    #[test]
    fn cursor_is_single_use_and_query_bound() {
        let store = SearchCursorStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(cursor()).expect("cursor should insert");
        let mut changed = binding();
        changed.keyword = "other".to_owned();
        assert!(matches!(
            store.take_for_query(&token, &changed),
            Err(SearchCursorError::QueryMismatch)
        ));
        assert!(store.take_for_query(&token, &binding()).is_ok());
        assert!(matches!(
            store.take_for_query(&token, &binding()),
            Err(SearchCursorError::UnknownOrExpired)
        ));
    }
}
