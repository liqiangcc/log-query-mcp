#![forbid(unsafe_code)]

mod mcp_server;
mod model;
mod scan_executor;
mod scanner;
mod time_filter;

#[cfg(target_os = "linux")]
mod context_reader;
#[cfg(target_os = "linux")]
mod match_reference;
#[cfg(target_os = "linux")]
mod safe_fs;
#[cfg(target_os = "linux")]
mod search_cursor;

pub use mcp_server::LogQueryServer;
pub use model::*;
pub use scan_executor::{ScanExecutor, ScanTaskError};
pub use scanner::{
    MAX_LINE_PREVIEW_BYTES, MAX_READ_BUFFER_BYTES, MAX_RETURNED_CONTENT_BYTES, MAX_SCAN_RESULTS,
    ScanError, ScanLimits, ScanMatch, ScanOutcome, ScanRequest, ScanStopReason, scan_reader,
};
pub use time_filter::{
    LineTimestamp, MAX_ROTATION_COMPONENT_CHARS, MAX_TIMESTAMP_FORMAT_CHARS,
    MAX_TIMESTAMP_PREFIX_BYTES, OrderedFileCandidate, RotationTimestampRule, TimeFilterDecision,
    TimeFilterError, TimeRange, TimedLogResult, TimestampRule, TimestampTracker,
    sort_file_candidates, sort_timed_results,
};

#[cfg(target_os = "linux")]
pub use context_reader::{
    ContextReadError, ContextReadLimits, ContextReadLine, ContextReadOutcome,
    MAX_CONTEXT_BACKTRACK_BYTES, MAX_CONTEXT_FORWARD_BYTES, read_referenced_context,
};
#[cfg(target_os = "linux")]
pub use match_reference::{
    MatchReferenceData, MatchReferenceError, MatchReferenceFileError, MatchReferenceStore,
    open_referenced_file,
};
#[cfg(target_os = "linux")]
pub use safe_fs::{FileIdentity, SafeFile, SafeOpenError, SafeRoot};
#[cfg(target_os = "linux")]
pub use search_cursor::{
    CursorCandidateFile, MAX_CURSOR_CANDIDATE_FILES, SearchCursorData, SearchCursorError,
    SearchCursorFileError, SearchCursorLease, SearchCursorQuery, SearchCursorStore,
    open_cursor_file,
};
