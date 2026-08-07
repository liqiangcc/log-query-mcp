mod gc;
mod generation;
mod manifest;
mod store;

pub use generation::{CacheFileId, CacheSourceId, GenerationId, GenerationKey};
pub use manifest::{
    ByteRange, CACHE_CATALOG_VERSION, CACHE_MANIFEST_VERSION, CacheCoverage, CacheManifest,
    GenerationMetadata, GenerationRecord, ManifestValidationError,
};
pub use store::{
    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,
    StagedGeneration,
};
