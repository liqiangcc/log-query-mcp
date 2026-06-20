#![forbid(unsafe_code)]

pub mod config;
mod scan_executor;
mod scanner;
mod time_filter;

#[cfg(target_os = "linux")]
mod query_engine;
#[cfg(target_os = "linux")]
mod safe_fs;
#[cfg(target_os = "linux")]
mod source_discovery;
#[cfg(target_os = "linux")]
mod source_registry;

pub use config::{
    AppConfig, CONFIG_VERSION, ConfigLoadError, ConfigValidationError, DirectoryRule, Encoding,
    LimitsConfig, LogSourceConfig, TimestampRule, ValidationIssue,
};
pub use scan_executor::{MAX_CONCURRENT_SCAN_TASKS, ScanExecutor, ScanTaskError};
pub use scanner::{
    MAX_LINE_PREVIEW_BYTES, MAX_READ_BUFFER_BYTES, MAX_RETURNED_CONTENT_BYTES, MAX_SCAN_BYTES,
    MAX_SCAN_KEYWORD_CHARS, MAX_SCAN_RESULTS, ScanError, ScanLimits, ScanMatch, ScanOutcome,
    ScanPosition, ScanRequest, ScanStopReason, scan_reader,
};
pub use time_filter::{
    MAX_TIMESTAMP_FORMAT_CHARS, MAX_TIMESTAMP_PREFIX_BYTES, TimeFilterDecision, TimeFilterError,
    TimeRange, TimestampObservation, TimestampParser,
};

#[cfg(target_os = "linux")]
pub use query_engine::{
    QueryEngine, QueryError, QueryMatch, QueryPage, QueryPageStopReason, QueryRequest, QuerySummary,
};
#[cfg(target_os = "linux")]
pub use safe_fs::{FileIdentity, SafeFile, SafeOpenError, SafeRoot};
#[cfg(target_os = "linux")]
pub(crate) use source_discovery::discover_regular_files;
#[cfg(target_os = "linux")]
pub use source_discovery::{DirectoryDiscoveryRule, SourceDiscoveryError};
#[cfg(target_os = "linux")]
pub use source_registry::{
    ConfiguredSource, MAX_REGISTERED_FILES_PER_SOURCE, SourceDescriptor, SourceFileSnapshot,
    SourceRegistry, SourceRegistryError,
};
