from pathlib import Path


def replace_once(path: str, old: str, new: str, label: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    file.write_text(text.replace(old, new))


replace_once(
    "src/source_registry.rs",
    '''    #[error("configured source backend requires asynchronous query preparation")]\n    AsyncBackendRequired,''',
    '''    #[error("configured source backend is not available yet: {source_id}/{backend}")]\n    BackendUnavailable {\n        source_id: String,\n        backend: &'static str,\n    },\n\n    #[error("configured source backend requires asynchronous query preparation")]\n    AsyncBackendRequired,''',
    "preserve BackendUnavailable compatibility",
)

replace_once(
    "src/source_registry.rs",
    '''    LimitsConfig, SafeFile, SafeOpenError, SourceDiscoveryError, SshConnectionManager,\n    SshTransportError, SyncEngine, SyncError, TimestampRule,''',
    '''    LimitsConfig, SafeFile, SafeOpenError, SourceDiscoveryError, SyncEngine, SyncError,\n    TimestampRule,''',
    "source registry root transport imports",
)
replace_once(
    "src/source_registry.rs",
    '''    backend::{LocalBackend, RemoteBackend, SnapshotFile, SourceBackend},''',
    '''    backend::{LocalBackend, RemoteBackend, SnapshotFile, SourceBackend},\n    transport::{SshConnectionManager, SshTransportError},''',
    "source registry transport module imports",
)

replace_once(
    "src/backend/remote.rs",
    '''    CacheStore, FileIdentity, GenerationId, GenerationPin, LogSourceConfigV2, RemoteFileType,\n    RemoteSyncTarget, SourceRegistryError, SshConnectionManager, SyncEngine,\n};''',
    '''    CacheStore, FileIdentity, GenerationId, GenerationPin, LogSourceConfigV2, RemoteSyncTarget,\n    SourceRegistryError, SyncEngine,\n    transport::{RemoteFileType, SshConnectionManager, SshReadTransport},\n};''',
    "remote backend transport imports",
)
replace_once(
    "src/backend/remote.rs",
    '''        reader: &crate::SshReadTransport,''',
    '''        reader: &SshReadTransport,''',
    "remote backend reader type",
)

replace_once(
    "src/backend/mod.rs",
    '''#[derive(Debug)]\npub enum SnapshotFile {''',
    '''pub enum SnapshotFile {''',
    "remove derived SnapshotFile Debug",
)
replace_once(
    "src/backend/mod.rs",
    '''impl SnapshotFile {\n    #[must_use]''',
    '''impl std::fmt::Debug for SnapshotFile {\n    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n        match self {\n            Self::Local(file) => formatter\n                .debug_struct("SnapshotFile::Local")\n                .field("identity", &file.identity())\n                .field("size", &file.size())\n                .finish(),\n            Self::Remote { identity, size, .. } => formatter\n                .debug_struct("SnapshotFile::Remote")\n                .field("identity", identity)\n                .field("size", size)\n                .finish(),\n        }\n    }\n}\n\nimpl SnapshotFile {\n    #[must_use]''',
    "manual SnapshotFile Debug",
)

replace_once(
    "src/tool_error.rs",
    '''            SourceRegistryError::InvalidConfiguration(_)\n            | SourceRegistryError::InvalidV2Configuration(_)\n            | SourceRegistryError::BackendUnavailable { .. }\n            | SourceRegistryError::DirectoryRuleInvalid { .. }\n            | SourceRegistryError::SnapshotSourceMismatch\n            | SourceRegistryError::PathNotConfigured => Self::internal_error(),\n            SourceRegistryError::RootUnavailable { source, .. }\n            | SourceRegistryError::ExplicitFileUnavailable { source, .. }\n            | SourceRegistryError::FileUnavailable { source, .. } => map_safe_open(source),\n            SourceRegistryError::DiscoveryFailed { source, .. } => map_source_discovery(source),\n            SourceRegistryError::TooManyFiles { .. } => Self::resource_limit(),\n            SourceRegistryError::UnknownSource(_) => Self::new(ToolErrorCode::UnknownSource),\n            SourceRegistryError::FileChanged { .. } => Self::new(ToolErrorCode::FileChanged),''',
    '''            SourceRegistryError::InvalidConfiguration(_)\n            | SourceRegistryError::InvalidV2Configuration(_)\n            | SourceRegistryError::BackendUnavailable { .. }\n            | SourceRegistryError::AsyncBackendRequired\n            | SourceRegistryError::RemoteConfigurationInvalid\n            | SourceRegistryError::RemoteRecursiveDiscoveryUnsupported { .. }\n            | SourceRegistryError::RemotePathInvalid\n            | SourceRegistryError::RemoteSnapshotMissingPin\n            | SourceRegistryError::CacheInitialization(_)\n            | SourceRegistryError::TransportInitialization(_)\n            | SourceRegistryError::SyncInitialization(_)\n            | SourceRegistryError::RemoteTaskJoin { .. }\n            | SourceRegistryError::DirectoryRuleInvalid { .. }\n            | SourceRegistryError::SnapshotSourceMismatch\n            | SourceRegistryError::PathNotConfigured => Self::internal_error(),\n            SourceRegistryError::RootUnavailable { source, .. }\n            | SourceRegistryError::ExplicitFileUnavailable { source, .. }\n            | SourceRegistryError::FileUnavailable { source, .. } => map_safe_open(source),\n            SourceRegistryError::DiscoveryFailed { source, .. } => map_source_discovery(source),\n            SourceRegistryError::TooManyFiles { .. } => Self::resource_limit(),\n            SourceRegistryError::UnknownSource(_) => Self::new(ToolErrorCode::UnknownSource),\n            SourceRegistryError::FileChanged { .. }\n            | SourceRegistryError::CachedGenerationUnavailable { .. } => {\n                Self::new(ToolErrorCode::FileChanged)\n            }\n            SourceRegistryError::RemoteExplicitFileNotRegular { .. }\n            | SourceRegistryError::RemoteTransport { .. }\n            | SourceRegistryError::RemoteSync { .. } => {\n                Self::new(ToolErrorCode::SourceUnavailable)\n            }''',
    "foundation source registry error mapping",
)
