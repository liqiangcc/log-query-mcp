from pathlib import Path


def replace_once(path: str, old: str, new: str, label: str) -> None:
    file = Path(path)
    text = file.read_text()
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected one match, got {count}")
    file.write_text(text.replace(old, new))


# Source snapshot coverage completeness is explicit and transport-independent.
replace_once(
    "src/source_registry.rs",
    '''    pub fn coverage(&self) -> Option<&CacheCoverage> {\n        self.coverage.as_ref()\n    }\n\n    #[must_use]\n    pub fn generation_pin(&self) -> Option<&GenerationPin> {''',
    '''    pub fn coverage(&self) -> Option<&CacheCoverage> {\n        self.coverage.as_ref()\n    }\n\n    #[must_use]\n    pub fn has_complete_coverage(&self) -> bool {\n        match self.coverage.as_ref() {\n            None | Some(CacheCoverage::Full) => true,\n            Some(CacheCoverage::Tail { start_offset })\n            | Some(CacheCoverage::FromNow { start_offset }) => *start_offset == 0,\n        }\n    }\n\n    #[must_use]\n    pub fn generation_pin(&self) -> Option<&GenerationPin> {''',
    "snapshot coverage completeness",
)

# Stateless query rejects partial cache before scanning.
replace_once(
    "src/query_engine.rs",
    '''        let snapshots = source.query_snapshot_files(remaining).await?;\n        let timestamp_parser = source''',
    '''        let snapshots = source.query_snapshot_files(remaining).await?;\n        if snapshots.iter().any(|snapshot| !snapshot.has_complete_coverage()) {\n            return Err(QueryError::CacheScopeExceeded);\n        }\n        let timestamp_parser = source''',
    "query engine coverage guard",
)
replace_once(
    "src/query_engine.rs",
    '''    #[error("query resource counter overflowed")]\n    ResourceCounterOverflow,\n\n    #[error(transparent)]\n    TimeFilter''',
    '''    #[error("query cannot prove that the local cache covers the requested remote log scope")]\n    CacheScopeExceeded,\n\n    #[error("query resource counter overflowed")]\n    ResourceCounterOverflow,\n\n    #[error(transparent)]\n    TimeFilter''',
    "query engine coverage error",
)

# Stateful query applies coverage guard only when building first-page candidates.
replace_once(
    "src/stateful_query.rs",
    '''        let snapshots = source.query_snapshot_files(remaining).await?;\n        remaining = remaining''',
    '''        let snapshots = source.query_snapshot_files(remaining).await?;\n        if snapshots.iter().any(|snapshot| !snapshot.has_complete_coverage()) {\n            return Err(StatefulQueryError::CacheScopeExceeded);\n        }\n        remaining = remaining''',
    "stateful query coverage guard",
)
replace_once(
    "src/stateful_query.rs",
    '''    #[error("query resource counter overflowed")]\n    ResourceCounterOverflow,\n\n    #[error(transparent)]\n    QueryState''',
    '''    #[error("query cannot prove that the local cache covers the requested remote log scope")]\n    CacheScopeExceeded,\n\n    #[error("query resource counter overflowed")]\n    ResourceCounterOverflow,\n\n    #[error(transparent)]\n    QueryState''',
    "stateful coverage error",
)

# Runtime ToolError now matches the already-frozen v2 schema.
replace_once(
    "src/tool_error.rs",
    '''    ContextReadError, ContextTaskError, QueryStateError, SafeOpenError, ScanError, ScanTaskError,\n    SourceDiscoveryError, SourceRegistryError, StatefulContextError, StatefulQueryError,\n    TimeFilterError,\n};''',
    '''    CacheStoreError, ContextReadError, ContextTaskError, QueryStateError, SafeOpenError,\n    ScanError, ScanTaskError, SourceDiscoveryError, SourceRegistryError, StatefulContextError,\n    StatefulQueryError, SyncError, TimeFilterError, transport::SshTransportError,\n};''',
    "tool error v2 imports",
)
replace_once(
    "src/tool_error.rs",
    '''    MatchRefInvalid,\n    FileChanged,\n    InternalError,''',
    '''    MatchRefInvalid,\n    FileChanged,\n    RemoteUnavailable,\n    RemoteAuthFailed,\n    HostKeyVerificationFailed,\n    RemoteFileChanged,\n    SyncFailed,\n    CacheScopeExceeded,\n    CacheLimitExceeded,\n    CacheCorrupted,\n    InternalError,''',
    "tool error v2 enum variants",
)
replace_once(
    "src/tool_error.rs",
    '''            Self::MatchRefInvalid => "MATCH_REF_INVALID",\n            Self::FileChanged => "FILE_CHANGED",\n            Self::InternalError => "INTERNAL_ERROR",''',
    '''            Self::MatchRefInvalid => "MATCH_REF_INVALID",\n            Self::FileChanged => "FILE_CHANGED",\n            Self::RemoteUnavailable => "REMOTE_UNAVAILABLE",\n            Self::RemoteAuthFailed => "REMOTE_AUTH_FAILED",\n            Self::HostKeyVerificationFailed => "HOST_KEY_VERIFICATION_FAILED",\n            Self::RemoteFileChanged => "REMOTE_FILE_CHANGED",\n            Self::SyncFailed => "SYNC_FAILED",\n            Self::CacheScopeExceeded => "CACHE_SCOPE_EXCEEDED",\n            Self::CacheLimitExceeded => "CACHE_LIMIT_EXCEEDED",\n            Self::CacheCorrupted => "CACHE_CORRUPTED",\n            Self::InternalError => "INTERNAL_ERROR",''',
    "tool error v2 wire codes",
)
replace_once(
    "src/tool_error.rs",
    '''            Self::FileChanged => "the referenced log file changed; run the search again",\n            Self::InternalError => "an internal error occurred; check the service logs",''',
    '''            Self::FileChanged => "the referenced log file changed; run the search again",\n            Self::RemoteUnavailable => "the remote log source is temporarily unavailable",\n            Self::RemoteAuthFailed => "remote log source authentication failed",\n            Self::HostKeyVerificationFailed => "remote host identity verification failed",\n            Self::RemoteFileChanged => "the remote log changed during synchronization; retry the search",\n            Self::SyncFailed => "the remote log could not be synchronized safely",\n            Self::CacheScopeExceeded => {\n                "the local cache does not fully cover the requested remote log scope"\n            }\n            Self::CacheLimitExceeded => "the remote log cache exceeded its configured capacity",\n            Self::CacheCorrupted => "the local remote-log cache is inconsistent or corrupted",\n            Self::InternalError => "an internal error occurred; check the service logs",''',
    "tool error v2 messages",
)
replace_once(
    "src/tool_error.rs",
    '''            Self::InvalidArgument\n            | Self::UnknownSource\n            | Self::ResourceLimit\n            | Self::CursorInvalid\n            | Self::MatchRefInvalid => false,\n            Self::SourceUnavailable\n            | Self::DeadlineExceeded\n            | Self::QueryCancelled\n            | Self::FileChanged\n            | Self::InternalError => true,''',
    '''            Self::InvalidArgument\n            | Self::UnknownSource\n            | Self::ResourceLimit\n            | Self::CursorInvalid\n            | Self::MatchRefInvalid\n            | Self::RemoteAuthFailed\n            | Self::HostKeyVerificationFailed\n            | Self::CacheScopeExceeded\n            | Self::CacheLimitExceeded\n            | Self::CacheCorrupted => false,\n            Self::SourceUnavailable\n            | Self::DeadlineExceeded\n            | Self::QueryCancelled\n            | Self::FileChanged\n            | Self::RemoteUnavailable\n            | Self::RemoteFileChanged\n            | Self::SyncFailed\n            | Self::InternalError => true,''',
    "tool error v2 retryability",
)
replace_once(
    "src/tool_error.rs",
    '''            StatefulQueryError::UnsafeContinuation\n            | StatefulQueryError::ResourceCounterOverflow => Self::internal_error(),''',
    '''            StatefulQueryError::CacheScopeExceeded => {\n                Self::new(ToolErrorCode::CacheScopeExceeded)\n            }\n            StatefulQueryError::UnsafeContinuation\n            | StatefulQueryError::ResourceCounterOverflow => Self::internal_error(),''',
    "stateful cache scope mapping",
)
replace_once(
    "src/tool_error.rs",
    '''            SourceRegistryError::InvalidConfiguration(_)\n            | SourceRegistryError::InvalidV2Configuration(_)\n            | SourceRegistryError::BackendUnavailable { .. }\n            | SourceRegistryError::AsyncBackendRequired\n            | SourceRegistryError::RemoteConfigurationInvalid\n            | SourceRegistryError::RemoteRecursiveDiscoveryUnsupported { .. }\n            | SourceRegistryError::RemotePathInvalid\n            | SourceRegistryError::RemoteSnapshotMissingPin\n            | SourceRegistryError::CacheInitialization(_)\n            | SourceRegistryError::TransportInitialization(_)\n            | SourceRegistryError::SyncInitialization(_)\n            | SourceRegistryError::RemoteTaskJoin { .. }\n            | SourceRegistryError::DirectoryRuleInvalid { .. }\n            | SourceRegistryError::SnapshotSourceMismatch\n            | SourceRegistryError::PathNotConfigured => Self::internal_error(),''',
    '''            SourceRegistryError::InvalidConfiguration(_)\n            | SourceRegistryError::InvalidV2Configuration(_)\n            | SourceRegistryError::BackendUnavailable { .. }\n            | SourceRegistryError::AsyncBackendRequired\n            | SourceRegistryError::RemoteConfigurationInvalid\n            | SourceRegistryError::RemoteRecursiveDiscoveryUnsupported { .. }\n            | SourceRegistryError::RemotePathInvalid\n            | SourceRegistryError::RemoteSnapshotMissingPin\n            | SourceRegistryError::TransportInitialization(_)\n            | SourceRegistryError::SyncInitialization(_)\n            | SourceRegistryError::RemoteTaskJoin { .. }\n            | SourceRegistryError::DirectoryRuleInvalid { .. }\n            | SourceRegistryError::SnapshotSourceMismatch\n            | SourceRegistryError::PathNotConfigured => Self::internal_error(),\n            SourceRegistryError::CacheInitialization(source) => map_cache_store(source),''',
    "source registry cache initialization mapping",
)
replace_once(
    "src/tool_error.rs",
    '''            SourceRegistryError::FileChanged { .. }\n            | SourceRegistryError::CachedGenerationUnavailable { .. } => {\n                Self::new(ToolErrorCode::FileChanged)\n            }\n            SourceRegistryError::RemoteExplicitFileNotRegular { .. }\n            | SourceRegistryError::RemoteTransport { .. }\n            | SourceRegistryError::RemoteSync { .. } => Self::new(ToolErrorCode::SourceUnavailable),''',
    '''            SourceRegistryError::FileChanged { .. } => Self::new(ToolErrorCode::FileChanged),\n            SourceRegistryError::CachedGenerationUnavailable { source, .. } => {\n                map_cache_store(source)\n            }\n            SourceRegistryError::RemoteExplicitFileNotRegular { .. } => {\n                Self::new(ToolErrorCode::SyncFailed)\n            }\n            SourceRegistryError::RemoteTransport { source, .. } => map_ssh_transport(source),\n            SourceRegistryError::RemoteSync { source, .. } => map_sync(source),''',
    "source registry v2 runtime mappings",
)

# Add exact transport/sync/cache classification helpers before legacy safe-open mapping.
replace_once(
    "src/tool_error.rs",
    '''#[cfg(target_os = "linux")]\nfn map_safe_open(_error: SafeOpenError) -> ToolError {\n    ToolError::new(ToolErrorCode::SourceUnavailable)\n}\n''',
    '''#[cfg(target_os = "linux")]\nfn map_ssh_transport(error: SshTransportError) -> ToolError {\n    match error {\n        SshTransportError::AuthenticationFailed => ToolError::new(ToolErrorCode::RemoteAuthFailed),\n        SshTransportError::HostKeyVerificationFailed => {\n            ToolError::new(ToolErrorCode::HostKeyVerificationFailed)\n        }\n        SshTransportError::ConnectTimeout\n        | SshTransportError::ConnectFailed\n        | SshTransportError::ConnectionLimit\n        | SshTransportError::OperationTimeout\n        | SshTransportError::SshProtocol\n        | SshTransportError::SftpProtocol\n        | SshTransportError::Broken => ToolError::new(ToolErrorCode::RemoteUnavailable),\n        SshTransportError::InvalidConfiguration\n        | SshTransportError::UnknownConnection\n        | SshTransportError::ConnectionManagerClosed\n        | SshTransportError::SecretUnavailable\n        | SshTransportError::KeyLoadFailed\n        | SshTransportError::InvalidRemotePath\n        | SshTransportError::InvalidReadRange => ToolError::internal_error(),\n    }\n}\n\n#[cfg(target_os = "linux")]\nfn map_sync(error: SyncError) -> ToolError {\n    match error {\n        SyncError::RemoteChangedDuringSync => ToolError::new(ToolErrorCode::RemoteFileChanged),\n        SyncError::Transport(error) => map_ssh_transport(error),\n        SyncError::Cache(error) => map_cache_store(error),\n        SyncError::CacheCapacityExceeded => ToolError::new(ToolErrorCode::CacheLimitExceeded),\n        SyncError::SyncLimitExceeded => ToolError::resource_limit(),\n        SyncError::InvalidConfiguration | SyncError::InvalidTarget => ToolError::internal_error(),\n        SyncError::RemoteFileNotRegular | SyncError::RemoteSizeUnavailable => {\n            ToolError::new(ToolErrorCode::SyncFailed)\n        }\n        SyncError::LocalIo(_) => ToolError::new(ToolErrorCode::CacheCorrupted),\n    }\n}\n\n#[cfg(target_os = "linux")]\nfn map_cache_store(error: CacheStoreError) -> ToolError {\n    match error {\n        CacheStoreError::CacheLimitExceeded => ToolError::new(ToolErrorCode::CacheLimitExceeded),\n        CacheStoreError::InvalidLimits\n        | CacheStoreError::InvalidSourceIdentifier\n        | CacheStoreError::StagingClosed\n        | CacheStoreError::AppendRangeMismatch\n        | CacheStoreError::ConcurrentGenerationChanged\n        | CacheStoreError::StatePoisoned\n        | CacheStoreError::ProtectedGenerationSelected\n        | CacheStoreError::InvalidSystemTime => ToolError::internal_error(),\n        CacheStoreError::Io(_)\n        | CacheStoreError::Json(_)\n        | CacheStoreError::Manifest(_)\n        | CacheStoreError::InvalidLayout\n        | CacheStoreError::ManifestIdentityMismatch\n        | CacheStoreError::GenerationNotFound\n        | CacheStoreError::GenerationLengthMismatch { .. } => {\n            ToolError::new(ToolErrorCode::CacheCorrupted)\n        }\n    }\n}\n\n#[cfg(target_os = "linux")]\nfn map_safe_open(_error: SafeOpenError) -> ToolError {\n    ToolError::new(ToolErrorCode::SourceUnavailable)\n}\n\n#[cfg(all(test, target_os = "linux"))]\nmod v2_error_tests {\n    use super::*;\n\n    #[test]\n    fn v2_wire_codes_match_the_frozen_schema() {\n        assert_eq!(ToolErrorCode::RemoteUnavailable.wire_code(), "REMOTE_UNAVAILABLE");\n        assert_eq!(ToolErrorCode::RemoteAuthFailed.wire_code(), "REMOTE_AUTH_FAILED");\n        assert_eq!(\n            ToolErrorCode::HostKeyVerificationFailed.wire_code(),\n            "HOST_KEY_VERIFICATION_FAILED"\n        );\n        assert_eq!(ToolErrorCode::RemoteFileChanged.wire_code(), "REMOTE_FILE_CHANGED");\n        assert_eq!(ToolErrorCode::SyncFailed.wire_code(), "SYNC_FAILED");\n        assert_eq!(ToolErrorCode::CacheScopeExceeded.wire_code(), "CACHE_SCOPE_EXCEEDED");\n        assert_eq!(ToolErrorCode::CacheLimitExceeded.wire_code(), "CACHE_LIMIT_EXCEEDED");\n        assert_eq!(ToolErrorCode::CacheCorrupted.wire_code(), "CACHE_CORRUPTED");\n    }\n\n    #[test]\n    fn v2_runtime_errors_keep_distinct_security_and_cache_classes() {\n        assert_eq!(\n            map_ssh_transport(SshTransportError::AuthenticationFailed).code,\n            ToolErrorCode::RemoteAuthFailed\n        );\n        assert_eq!(\n            map_ssh_transport(SshTransportError::HostKeyVerificationFailed).code,\n            ToolErrorCode::HostKeyVerificationFailed\n        );\n        assert_eq!(\n            map_sync(SyncError::RemoteChangedDuringSync).code,\n            ToolErrorCode::RemoteFileChanged\n        );\n        assert_eq!(\n            map_cache_store(CacheStoreError::CacheLimitExceeded).code,\n            ToolErrorCode::CacheLimitExceeded\n        );\n        assert_eq!(\n            ToolError::from(StatefulQueryError::CacheScopeExceeded).code,\n            ToolErrorCode::CacheScopeExceeded\n        );\n    }\n}\n''',
    "v2 runtime error helpers and tests",
)
