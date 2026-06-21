use std::time::Duration;

use thiserror::Error;

use crate::{
    QueryMatch, SourceFileSnapshot, state_store::{ExpiringStore, StateStoreError},
};

const MATCH_REFERENCE_PREFIX: &str = "mref_";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchReferenceData {
    snapshot: SourceFileSnapshot,
    line_number: u64,
    line_start_offset: u64,
    match_byte_offset: u64,
    keyword: String,
    case_sensitive: bool,
}

impl MatchReferenceData {
    pub fn from_query_match(
        snapshot: SourceFileSnapshot,
        query_match: &QueryMatch,
        keyword: impl Into<String>,
        case_sensitive: bool,
    ) -> Result<Self, MatchReferenceError> {
        if snapshot.source_id() != query_match.source_id
            || snapshot.file_id() != query_match.file_id
        {
            return Err(MatchReferenceError::InvalidData(
                "query match does not belong to the supplied file snapshot",
            ));
        }
        let value = Self {
            snapshot,
            line_number: query_match.line_number,
            line_start_offset: query_match.line_start_offset,
            match_byte_offset: query_match.match_byte_offset,
            keyword: keyword.into(),
            case_sensitive,
        };
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<(), MatchReferenceError> {
        if self.line_number == 0 || self.line_start_offset > self.match_byte_offset {
            return Err(MatchReferenceError::InvalidData(
                "match line number or byte offsets are invalid",
            ));
        }
        let keyword = self.keyword.as_bytes();
        if keyword.is_empty()
            || self.keyword.chars().count() > 256
            || keyword.contains(&b'\n')
            || keyword.contains(&b'\r')
        {
            return Err(MatchReferenceError::InvalidData(
                "match keyword is invalid",
            ));
        }
        let keyword_len = u64::try_from(keyword.len())
            .map_err(|_| MatchReferenceError::InvalidData("keyword length is not representable"))?;
        let match_end = self
            .match_byte_offset
            .checked_add(keyword_len)
            .ok_or(MatchReferenceError::InvalidData(
                "match byte range overflows",
            ))?;
        if match_end > self.snapshot.size_at_snapshot() {
            return Err(MatchReferenceError::InvalidData(
                "match byte range exceeds the file snapshot",
            ));
        }
        Ok(())
    }

    #[must_use]
    pub const fn snapshot(&self) -> &SourceFileSnapshot {
        &self.snapshot
    }

    #[must_use]
    pub const fn line_number(&self) -> u64 {
        self.line_number
    }

    #[must_use]
    pub const fn line_start_offset(&self) -> u64 {
        self.line_start_offset
    }

    #[must_use]
    pub const fn match_byte_offset(&self) -> u64 {
        self.match_byte_offset
    }

    #[must_use]
    pub fn keyword(&self) -> &str {
        &self.keyword
    }

    #[must_use]
    pub const fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }
}

#[derive(Debug)]
pub struct MatchReferenceStore {
    inner: ExpiringStore<MatchReferenceData>,
}

impl MatchReferenceStore {
    pub fn new(capacity: usize, ttl: Duration) -> Result<Self, MatchReferenceError> {
        Ok(Self {
            inner: ExpiringStore::new(MATCH_REFERENCE_PREFIX, capacity, ttl)?,
        })
    }

    pub fn insert(&self, value: MatchReferenceData) -> Result<String, MatchReferenceError> {
        value.validate()?;
        Ok(self.inner.insert(value)?)
    }

    pub fn resolve(&self, token: &str) -> Result<MatchReferenceData, MatchReferenceError> {
        Ok(self.inner.get_cloned(token)?)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[derive(Debug, Error)]
pub enum MatchReferenceError {
    #[error("invalid match reference data: {0}")]
    InvalidData(&'static str),

    #[error("unknown or expired match reference")]
    UnknownOrExpired,

    #[error("match reference store configuration is invalid")]
    InvalidStore,
}

impl From<StateStoreError> for MatchReferenceError {
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

    fn snapshot() -> SourceFileSnapshot {
        SourceFileSnapshot::from_parts_for_state(
            "payment-test".to_owned(),
            "file_test".to_owned(),
            PathBuf::from("application.log"),
            FileIdentity {
                device: 10,
                inode: 20,
            },
            4096,
        )
    }

    fn query_match() -> QueryMatch {
        QueryMatch {
            source_id: "payment-test".to_owned(),
            file_id: "file_test".to_owned(),
            file_name: "application.log".to_owned(),
            line_number: 42,
            timestamp: None,
            content: "traceId=abc123".to_owned(),
            content_truncated: false,
            content_lossy: false,
            original_line_bytes: 14,
            line_start_offset: 100,
            match_byte_offset: 108,
        }
    }

    #[test]
    fn creates_and_resolves_opaque_reference() {
        let store = MatchReferenceStore::new(10, Duration::from_secs(60))
            .expect("store should be created");
        let value = MatchReferenceData::from_query_match(
            snapshot(),
            &query_match(),
            "abc123",
            false,
        )
        .expect("reference data should be valid");
        let token = store.insert(value.clone()).expect("reference should insert");

        assert!(token.starts_with("mref_"));
        assert!(!token.contains("application"));
        assert_eq!(store.resolve(&token).expect("reference should resolve"), value);
    }

    #[test]
    fn rejects_inconsistent_match_and_snapshot() {
        let mut query_match = query_match();
        query_match.file_id = "other".to_owned();
        assert!(MatchReferenceData::from_query_match(
            snapshot(),
            &query_match,
            "abc123",
            false,
        )
        .is_err());
    }
}
