#![forbid(unsafe_code)]

mod mcp_server;
mod model;
mod scan_executor;
mod scanner;

#[cfg(target_os = "linux")]
mod context_reader;
#[cfg(target_os = "linux")]
mod match_reference;
#[cfg(target_os = "linux")]
mod safe_fs;

pub use mcp_server::LogQueryServer;
pub use model::*;
pub use scan_executor::{ScanExecutor, ScanTaskError};
pub use scanner::{
    MAX_LINE_PREVIEW_BYTES, MAX_READ_BUFFER_BYTES, MAX_RETURNED_CONTENT_BYTES, MAX_SCAN_RESULTS,
    ScanError, ScanLimits, ScanMatch, ScanOutcome, ScanRequest, ScanStopReason, scan_reader,
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
