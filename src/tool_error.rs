use serde::Serialize;

#[cfg(target_os = "linux")]
use crate::{
    ContextReadError, ContextTaskError, QueryStateError, SafeOpenError, ScanError, ScanTaskError,
    SourceDiscoveryError, SourceRegistryError, StatefulContextError, StatefulQueryError,
    TimeFilterError,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ToolErrorCode {
    InvalidArgument,
    UnknownSource,
    SourceUnavailable,
    DeadlineExceeded,
    QueryCancelled,
    ResourceLimit,
    CursorInvalid,
    MatchRefInvalid,
    FileChanged,
    InternalError,
}

impl ToolErrorCode {
    #[must_use]
    pub const fn wire_code(self) -> &'static str {
        match self {
            Self::InvalidArgument => "INVALID_ARGUMENT",
            Self::UnknownSource => "UNKNOWN_SOURCE",
            Self::SourceUnavailable => "SOURCE_UNAVAILABLE",
            Self::DeadlineExceeded => "DEADLINE_EXCEEDED",
            Self::QueryCancelled => "QUERY_CANCELLED",
            Self::ResourceLimit => "RESOURCE_LIMIT",
            Self::CursorInvalid => "CURSOR_INVALID",
            Self::MatchRefInvalid => "MATCH_REF_INVALID",
            Self::FileChanged => "FILE_CHANGED",
            Self::InternalError => "INTERNAL_ERROR",
        }
    }

    #[must_use]
    pub const fn default_message(self) -> &'static str {
        match self {
            Self::InvalidArgument => "invalid tool arguments",
            Self::UnknownSource => "one or more requested log sources are unavailable",
            Self::SourceUnavailable => {
                "one or more configured log files are temporarily unavailable"
            }
            Self::DeadlineExceeded => {
                "the request deadline was exceeded; narrow the query and try again"
            }
            Self::QueryCancelled => "the request was cancelled",
            Self::ResourceLimit => "the request exceeded a service resource limit",
            Self::CursorInvalid => "the search cursor is invalid or expired; run the search again",
            Self::MatchRefInvalid => {
                "the match reference is invalid or expired; run the search again"
            }
            Self::FileChanged => "the referenced log file changed; run the search again",
            Self::InternalError => "an internal error occurred; check the service logs",
        }
    }

    #[must_use]
    pub const fn retryable(self) -> bool {
        match self {
            Self::InvalidArgument
            | Self::UnknownSource
            | Self::ResourceLimit
            | Self::CursorInvalid
            | Self::MatchRefInvalid => false,
            Self::SourceUnavailable
            | Self::DeadlineExceeded
            | Self::QueryCancelled
            | Self::FileChanged
            | Self::InternalError => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolError {
    pub code: ToolErrorCode,
    pub message: String,
    pub retryable: bool,
}

impl ToolError {
    #[must_use]
    pub fn new(code: ToolErrorCode) -> Self {
        Self {
            code,
            message: code.default_message().to_owned(),
            retryable: code.retryable(),
        }
    }

    #[must_use]
    pub fn invalid_argument() -> Self {
        Self::new(ToolErrorCode::InvalidArgument)
    }

    #[must_use]
    pub fn resource_limit() -> Self {
        Self::new(ToolErrorCode::ResourceLimit)
    }

    #[must_use]
    pub fn internal_error() -> Self {
        Self::new(ToolErrorCode::InternalError)
    }

    pub fn to_json_string(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }
}

#[cfg(target_os = "linux")]
impl From<StatefulQueryError> for ToolError {
    fn from(error: StatefulQueryError) -> Self {
        match error {
            StatefulQueryError::InvalidArgument(_) => Self::invalid_argument(),
            StatefulQueryError::DeadlineOverflow => Self::internal_error(),
            StatefulQueryError::Cancelled => Self::new(ToolErrorCode::QueryCancelled),
            StatefulQueryError::DeadlineExceeded => Self::new(ToolErrorCode::DeadlineExceeded),
            StatefulQueryError::FileLimitExceeded => Self::resource_limit(),
            StatefulQueryError::InvalidCursorState => Self::new(ToolErrorCode::CursorInvalid),
            StatefulQueryError::InvalidScanPosition
            | StatefulQueryError::ScanPositionNotLineBoundary => {
                Self::new(ToolErrorCode::FileChanged)
            }
            StatefulQueryError::UnsafeContinuation
            | StatefulQueryError::ResourceCounterOverflow => Self::internal_error(),
            StatefulQueryError::QueryState(error) => map_query_state_for_cursor(error),
            StatefulQueryError::TimeFilter(error) => map_time_filter(error),
            StatefulQueryError::SourceRegistry(error) => Self::from(error),
            StatefulQueryError::ScanTask(error) => map_scan_task(error),
            StatefulQueryError::Io(_) => Self::new(ToolErrorCode::SourceUnavailable),
        }
    }
}

#[cfg(target_os = "linux")]
impl From<StatefulContextError> for ToolError {
    fn from(error: StatefulContextError) -> Self {
        match error {
            StatefulContextError::InvalidArgument(_) => Self::invalid_argument(),
            StatefulContextError::DeadlineOverflow => Self::internal_error(),
            StatefulContextError::Cancelled => Self::new(ToolErrorCode::QueryCancelled),
            StatefulContextError::DeadlineExceeded => Self::new(ToolErrorCode::DeadlineExceeded),
            StatefulContextError::QueryState(error) => map_query_state_for_match_ref(error),
            StatefulContextError::SourceRegistry(error) => Self::from(error),
            StatefulContextError::ContextRead(error) => Self::from(error),
            StatefulContextError::ContextTask(error) => map_context_task(error),
        }
    }
}

#[cfg(target_os = "linux")]
impl From<QueryStateError> for ToolError {
    fn from(error: QueryStateError) -> Self {
        map_query_state_for_cursor(error)
    }
}

#[cfg(target_os = "linux")]
impl From<SourceRegistryError> for ToolError {
    fn from(error: SourceRegistryError) -> Self {
        match error {
            SourceRegistryError::InvalidConfiguration(_)
            | SourceRegistryError::InvalidV2Configuration(_)
            | SourceRegistryError::BackendUnavailable { .. }
            | SourceRegistryError::DirectoryRuleInvalid { .. }
            | SourceRegistryError::SnapshotSourceMismatch
            | SourceRegistryError::PathNotConfigured => Self::internal_error(),
            SourceRegistryError::RootUnavailable { source, .. }
            | SourceRegistryError::ExplicitFileUnavailable { source, .. }
            | SourceRegistryError::FileUnavailable { source, .. } => map_safe_open(source),
            SourceRegistryError::DiscoveryFailed { source, .. } => map_source_discovery(source),
            SourceRegistryError::TooManyFiles { .. } => Self::resource_limit(),
            SourceRegistryError::UnknownSource(_) => Self::new(ToolErrorCode::UnknownSource),
            SourceRegistryError::FileChanged { .. } => Self::new(ToolErrorCode::FileChanged),
        }
    }
}

#[cfg(target_os = "linux")]
impl From<ContextReadError> for ToolError {
    fn from(error: ContextReadError) -> Self {
        match error {
            ContextReadError::InvalidRequest(_) => Self::invalid_argument(),
            ContextReadError::InvalidLimits(_) | ContextReadError::CounterOverflow => {
                Self::internal_error()
            }
            ContextReadError::FileChanged => Self::new(ToolErrorCode::FileChanged),
            ContextReadError::MatchOutsideScanBudget | ContextReadError::MatchLineScanLimit => {
                Self::resource_limit()
            }
            ContextReadError::Cancelled => Self::new(ToolErrorCode::QueryCancelled),
            ContextReadError::DeadlineExceeded => Self::new(ToolErrorCode::DeadlineExceeded),
            ContextReadError::InvalidReference(error) => map_query_state_for_match_ref(error),
            ContextReadError::SourceRegistry(error) => Self::from(error),
            ContextReadError::Io(_) => Self::new(ToolErrorCode::SourceUnavailable),
        }
    }
}

#[cfg(target_os = "linux")]
fn map_query_state_for_cursor(error: QueryStateError) -> ToolError {
    match error {
        QueryStateError::InvalidData(_) => ToolError::invalid_argument(),
        QueryStateError::CumulativeLimit | QueryStateError::CapacityBusy => {
            ToolError::resource_limit()
        }
        QueryStateError::QueryMismatch
        | QueryStateError::Busy
        | QueryStateError::LeaseLost
        | QueryStateError::InvalidContinuation(_)
        | QueryStateError::UnknownOrExpired => ToolError::new(ToolErrorCode::CursorInvalid),
        QueryStateError::InvalidCapacity
        | QueryStateError::InvalidTtl
        | QueryStateError::ExpirationOverflow
        | QueryStateError::CounterOverflow => ToolError::internal_error(),
    }
}

#[cfg(target_os = "linux")]
fn map_query_state_for_match_ref(error: QueryStateError) -> ToolError {
    match error {
        QueryStateError::UnknownOrExpired | QueryStateError::InvalidData(_) => {
            ToolError::new(ToolErrorCode::MatchRefInvalid)
        }
        QueryStateError::CumulativeLimit | QueryStateError::CapacityBusy => {
            ToolError::resource_limit()
        }
        QueryStateError::InvalidCapacity
        | QueryStateError::InvalidTtl
        | QueryStateError::ExpirationOverflow
        | QueryStateError::CounterOverflow
        | QueryStateError::QueryMismatch
        | QueryStateError::Busy
        | QueryStateError::LeaseLost
        | QueryStateError::InvalidContinuation(_) => ToolError::internal_error(),
    }
}

#[cfg(target_os = "linux")]
fn map_time_filter(error: TimeFilterError) -> ToolError {
    match error {
        TimeFilterError::InvalidRange(_) => ToolError::invalid_argument(),
        TimeFilterError::InvalidConfiguration(_) => ToolError::internal_error(),
    }
}

#[cfg(target_os = "linux")]
fn map_scan_task(error: ScanTaskError) -> ToolError {
    match error {
        ScanTaskError::InvalidConcurrency
        | ScanTaskError::ExecutorClosed
        | ScanTaskError::Join(_) => ToolError::internal_error(),
        ScanTaskError::Scan(error) => map_scan(error),
    }
}

#[cfg(target_os = "linux")]
fn map_scan(error: ScanError) -> ToolError {
    match error {
        ScanError::InvalidKeyword
        | ScanError::InvalidStartPosition
        | ScanError::InvalidLimits(_) => ToolError::invalid_argument(),
        ScanError::PositionOverflow => ToolError::internal_error(),
        ScanError::Io(_) => ToolError::new(ToolErrorCode::SourceUnavailable),
    }
}

#[cfg(target_os = "linux")]
fn map_context_task(error: ContextTaskError) -> ToolError {
    match error {
        ContextTaskError::InvalidConcurrency
        | ContextTaskError::ExecutorClosed
        | ContextTaskError::Join(_) => ToolError::internal_error(),
    }
}

#[cfg(target_os = "linux")]
fn map_source_discovery(error: SourceDiscoveryError) -> ToolError {
    match error {
        SourceDiscoveryError::InvalidRule(_) => ToolError::internal_error(),
        SourceDiscoveryError::TooManyEntries
        | SourceDiscoveryError::TooManyDirectories
        | SourceDiscoveryError::TooManyFiles => ToolError::resource_limit(),
        SourceDiscoveryError::DirectoryRead(_) => ToolError::new(ToolErrorCode::SourceUnavailable),
        SourceDiscoveryError::SafeOpen(error) => map_safe_open(error),
    }
}

#[cfg(target_os = "linux")]
fn map_safe_open(_error: SafeOpenError) -> ToolError {
    ToolError::new(ToolErrorCode::SourceUnavailable)
}
