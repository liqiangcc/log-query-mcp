#![forbid(unsafe_code)]

mod mcp_server;
mod model;

#[cfg(target_os = "linux")]
mod safe_fs;

pub use mcp_server::LogQueryServer;
pub use model::*;

#[cfg(target_os = "linux")]
pub use safe_fs::{FileIdentity, SafeFile, SafeOpenError, SafeRoot};
