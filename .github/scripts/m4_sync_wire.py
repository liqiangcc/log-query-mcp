from pathlib import Path


def replace_once(path: Path, old: str, new: str, label: str) -> None:
    text = path.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, found {count}")
    path.write_text(text.replace(old, new))


sync = Path("src/cache/sync.rs")
replace_once(
    sync,
    '''    use crate::{
        CacheStoreLimits, FreshnessPolicy, RemoteSyncPolicy, SourceBackendConfig,
        config_v2::SourceBackendConfig as _,
    };''',
    '''    use crate::CacheStoreLimits;''',
    "clean M4 test imports",
)

cargo = Path("Cargo.toml")
replace_once(
    cargo,
    'serde_json = "1"\nthiserror = "2"',
    'serde_json = "1"\nsha2 = "0.11"\nthiserror = "2"',
    "add sha2 dependency",
)

cache_mod = Path("src/cache/mod.rs")
replace_once(
    cache_mod,
    'mod store;\n',
    'mod store;\nmod sync;\n',
    "wire sync module",
)
replace_once(
    cache_mod,
    '''pub use store::{
    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,
    StagedAppend, StagedGeneration,
};''',
    '''pub use store::{
    CacheStore, CacheStoreError, CacheStoreLimits, GcReport, PinnedGeneration, RecoveryReport,
    StagedAppend, StagedGeneration,
};
pub use sync::{
    CONTINUITY_FINGERPRINT_WINDOW_BYTES, RemoteSyncTarget, SyncAction, SyncEngine, SyncError,
    SyncGenerationReason, SyncOutcome,
};''',
    "export sync module",
)

lib = Path("src/lib.rs")
replace_once(
    lib,
    '''    GenerationId, GenerationKey, GenerationMetadata, GenerationRecord, ManifestValidationError,
    PinnedGeneration, RecoveryReport, StagedAppend, StagedGeneration,
};''',
    '''    GenerationId, GenerationKey, GenerationMetadata, GenerationRecord, ManifestValidationError,
    PinnedGeneration, RecoveryReport, RemoteSyncTarget, StagedAppend, StagedGeneration, SyncAction,
    SyncEngine, SyncError, SyncGenerationReason, SyncOutcome, CONTINUITY_FINGERPRINT_WINDOW_BYTES,
};''',
    "export M4 crate API",
)
