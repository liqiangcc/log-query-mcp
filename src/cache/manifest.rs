use std::{collections::HashSet, path::Path};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::generation::{CacheFileId, CacheSourceId, GenerationId};

pub const CACHE_CATALOG_VERSION: u32 = 1;
pub const CACHE_MANIFEST_VERSION: u32 = 1;
const MAX_REMOTE_IDENTIFIER_CHARS: usize = 4096;
const MAX_FINGERPRINT_CHARS: usize = 512;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CacheCatalog {
    pub version: u32,
    pub sources: Vec<CatalogSource>,
}

impl Default for CacheCatalog {
    fn default() -> Self {
        Self {
            version: CACHE_CATALOG_VERSION,
            sources: Vec::new(),
        }
    }
}

impl CacheCatalog {
    pub(crate) fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.version != CACHE_CATALOG_VERSION {
            return Err(ManifestValidationError::UnsupportedCatalogVersion);
        }

        let mut source_identifiers = HashSet::new();
        let mut source_ids = HashSet::new();
        let mut file_ids = HashSet::new();
        for source in &self.sources {
            if source.source_identifier.is_empty()
                || !source_identifiers.insert(&source.source_identifier)
            {
                return Err(ManifestValidationError::InvalidCatalog);
            }
            validate_source_id(&source.cache_id)?;
            if !source_ids.insert(source.cache_id.as_str()) {
                return Err(ManifestValidationError::InvalidCatalog);
            }
            let mut remote_identifiers = HashSet::new();
            for file in &source.files {
                validate_remote_identifier(&file.remote_identifier)?;
                validate_file_id(&file.file_id)?;
                if !remote_identifiers.insert(&file.remote_identifier)
                    || !file_ids.insert(file.file_id.as_str())
                {
                    return Err(ManifestValidationError::InvalidCatalog);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogSource {
    pub source_identifier: String,
    pub cache_id: CacheSourceId,
    pub files: Vec<CatalogFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct CatalogFile {
    pub remote_identifier: String,
    pub file_id: CacheFileId,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ByteRange {
    pub start: u64,
    pub end_exclusive: u64,
}

impl ByteRange {
    pub fn new(start: u64, end_exclusive: u64) -> Result<Self, ManifestValidationError> {
        let range = Self {
            start,
            end_exclusive,
        };
        range.validate()?;
        Ok(range)
    }

    #[must_use]
    pub fn len(self) -> u64 {
        self.end_exclusive - self.start
    }

    #[must_use]
    pub fn is_empty(self) -> bool {
        self.start == self.end_exclusive
    }

    fn validate(self) -> Result<(), ManifestValidationError> {
        if self.end_exclusive < self.start {
            return Err(ManifestValidationError::InvalidRange);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CacheCoverage {
    Full,
    Tail { start_offset: u64 },
    FromNow { start_offset: u64 },
}

impl CacheCoverage {
    fn validate(&self, range: ByteRange) -> Result<(), ManifestValidationError> {
        match self {
            Self::Full => {
                if range.start != 0 {
                    return Err(ManifestValidationError::InvalidCoverage);
                }
            }
            Self::Tail { start_offset } | Self::FromNow { start_offset } => {
                if *start_offset != range.start {
                    return Err(ManifestValidationError::InvalidCoverage);
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GenerationMetadata {
    pub remote_size: u64,
    pub cached_range: ByteRange,
    pub remote_mtime_millis: Option<i64>,
    pub continuity_fingerprint: Option<String>,
    pub coverage: CacheCoverage,
}

impl GenerationMetadata {
    pub(crate) fn validate(&self) -> Result<(), ManifestValidationError> {
        self.cached_range.validate()?;
        if self.cached_range.end_exclusive > self.remote_size {
            return Err(ManifestValidationError::RangeExceedsRemoteSize);
        }
        self.coverage.validate(self.cached_range)?;
        if self.continuity_fingerprint.as_ref().is_some_and(|value| {
            value.is_empty()
                || value.len() > MAX_FINGERPRINT_CHARS
                || value.chars().any(char::is_control)
        }) {
            return Err(ManifestValidationError::InvalidFingerprint);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct GenerationRecord {
    pub generation: GenerationId,
    pub remote_size: u64,
    pub cached_range: ByteRange,
    pub remote_mtime_millis: Option<i64>,
    pub last_sync_unix_millis: u64,
    pub continuity_fingerprint: Option<String>,
    pub coverage: CacheCoverage,
    pub data_len: u64,
    pub created_at_unix_millis: u64,
}

impl GenerationRecord {
    pub(crate) fn validate(&self) -> Result<(), ManifestValidationError> {
        validate_generation_id(&self.generation)?;
        let metadata = GenerationMetadata {
            remote_size: self.remote_size,
            cached_range: self.cached_range,
            remote_mtime_millis: self.remote_mtime_millis,
            continuity_fingerprint: self.continuity_fingerprint.clone(),
            coverage: self.coverage.clone(),
        };
        metadata.validate()?;
        if self.data_len != self.cached_range.len() {
            return Err(ManifestValidationError::DataLengthMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CacheManifest {
    pub version: u32,
    pub source_identifier: String,
    pub source_id: CacheSourceId,
    pub file_id: CacheFileId,
    pub remote_identifier: String,
    pub current_generation: Option<GenerationId>,
    pub generations: Vec<GenerationRecord>,
    pub updated_at_unix_millis: u64,
}

impl CacheManifest {
    pub(crate) fn new(
        source_identifier: String,
        source_id: CacheSourceId,
        file_id: CacheFileId,
        remote_identifier: String,
        now_unix_millis: u64,
    ) -> Self {
        Self {
            version: CACHE_MANIFEST_VERSION,
            source_identifier,
            source_id,
            file_id,
            remote_identifier,
            current_generation: None,
            generations: Vec::new(),
            updated_at_unix_millis: now_unix_millis,
        }
    }

    pub fn validate(&self) -> Result<(), ManifestValidationError> {
        if self.version != CACHE_MANIFEST_VERSION {
            return Err(ManifestValidationError::UnsupportedManifestVersion);
        }
        if self.source_identifier.is_empty() {
            return Err(ManifestValidationError::InvalidManifest);
        }
        validate_source_id(&self.source_id)?;
        validate_file_id(&self.file_id)?;
        validate_remote_identifier(&self.remote_identifier)?;

        let mut ids = HashSet::new();
        for generation in &self.generations {
            generation.validate()?;
            if !ids.insert(generation.generation.as_str()) {
                return Err(ManifestValidationError::DuplicateGeneration);
            }
        }

        match &self.current_generation {
            Some(current)
                if self
                    .generations
                    .iter()
                    .any(|record| record.generation == *current) => {}
            Some(_) => return Err(ManifestValidationError::MissingCurrentGeneration),
            None if !self.generations.is_empty() => {
                return Err(ManifestValidationError::MissingCurrentGeneration);
            }
            None => {}
        }
        Ok(())
    }

    pub(crate) fn append_generation(&mut self, generation: GenerationRecord, now: u64) {
        self.current_generation = Some(generation.generation.clone());
        self.generations.push(generation);
        self.updated_at_unix_millis = now;
    }

    pub(crate) fn remove_generation(&mut self, generation: &GenerationId, now: u64) -> bool {
        if self.current_generation.as_ref() == Some(generation) {
            return false;
        }
        let before = self.generations.len();
        self.generations
            .retain(|record| &record.generation != generation);
        let changed = before != self.generations.len();
        if changed {
            self.updated_at_unix_millis = now;
        }
        changed
    }

    #[must_use]
    pub fn current(&self) -> Option<&GenerationRecord> {
        let current = self.current_generation.as_ref()?;
        self.generations
            .iter()
            .find(|record| &record.generation == current)
    }
}

pub(crate) fn validate_remote_identifier(value: &str) -> Result<(), ManifestValidationError> {
    if value.is_empty()
        || value.len() > MAX_REMOTE_IDENTIFIER_CHARS
        || value.chars().any(char::is_control)
    {
        return Err(ManifestValidationError::InvalidRemoteIdentifier);
    }
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(ManifestValidationError::InvalidRemoteIdentifier);
    }
    Ok(())
}

fn validate_source_id(value: &CacheSourceId) -> Result<(), ManifestValidationError> {
    CacheSourceId::parse(value.as_str().to_owned())
        .map(|_| ())
        .map_err(|_| ManifestValidationError::InvalidOpaqueId)
}

fn validate_file_id(value: &CacheFileId) -> Result<(), ManifestValidationError> {
    CacheFileId::parse(value.as_str().to_owned())
        .map(|_| ())
        .map_err(|_| ManifestValidationError::InvalidOpaqueId)
}

fn validate_generation_id(value: &GenerationId) -> Result<(), ManifestValidationError> {
    GenerationId::parse(value.as_str().to_owned())
        .map(|_| ())
        .map_err(|_| ManifestValidationError::InvalidOpaqueId)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ManifestValidationError {
    #[error("unsupported cache catalog version")]
    UnsupportedCatalogVersion,
    #[error("unsupported cache manifest version")]
    UnsupportedManifestVersion,
    #[error("cache catalog is invalid")]
    InvalidCatalog,
    #[error("cache manifest is invalid")]
    InvalidManifest,
    #[error("cache opaque identifier is invalid")]
    InvalidOpaqueId,
    #[error("remote identifier must be a safe relative identifier")]
    InvalidRemoteIdentifier,
    #[error("cached byte range is invalid")]
    InvalidRange,
    #[error("cached byte range exceeds remote size")]
    RangeExceedsRemoteSize,
    #[error("cache coverage is inconsistent with cached range")]
    InvalidCoverage,
    #[error("continuity fingerprint is invalid")]
    InvalidFingerprint,
    #[error("cached data length does not match cached range")]
    DataLengthMismatch,
    #[error("cache manifest contains a duplicate generation")]
    DuplicateGeneration,
    #[error("cache manifest current generation is missing")]
    MissingCurrentGeneration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_remote_path_escape() {
        assert_eq!(
            validate_remote_identifier("../secret.log"),
            Err(ManifestValidationError::InvalidRemoteIdentifier)
        );
        assert_eq!(
            validate_remote_identifier("/var/log/app.log"),
            Err(ManifestValidationError::InvalidRemoteIdentifier)
        );
        assert!(validate_remote_identifier("service/application.log").is_ok());
    }

    #[test]
    fn coverage_must_match_cached_range() {
        let metadata = GenerationMetadata {
            remote_size: 100,
            cached_range: ByteRange::new(50, 100).expect("range"),
            remote_mtime_millis: None,
            continuity_fingerprint: None,
            coverage: CacheCoverage::Tail { start_offset: 40 },
        };
        assert_eq!(
            metadata.validate(),
            Err(ManifestValidationError::InvalidCoverage)
        );
    }
}
