mod gc;
mod generation;
mod manifest;
mod store;
mod sync;

pub use generation::{CacheFileId, CacheSourceId, GenerationId, GenerationKey};
pub use manifest::{
    ByteRange, CACHE_CATALOG_VERSION, CACHE_MANIFEST_VERSION, CacheCoverage, CacheManifest,
    GenerationMetadata, GenerationRecord, ManifestValidationError,
};
pub use store::{
    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,
    StagedAppend, StagedGeneration,
};
pub use sync::{
    CONTINUITY_FINGERPRINT_WINDOW_BYTES, RemoteSyncTarget, SyncAction, SyncEngine, SyncError,
    SyncGenerationReason, SyncOutcome,
};
