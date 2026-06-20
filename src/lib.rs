#![forbid(unsafe_code)]

pub mod config;

#[cfg(target_os = "linux")]
pub mod safe_fs;
#[cfg(target_os = "linux")]
pub mod source_discovery;
#[cfg(target_os = "linux")]
pub mod source_registry;

pub use config::{
    AppConfig, ConfigLoadError, ConfigValidationError, DirectoryRule, Encoding, LimitsConfig,
    LogSourceConfig, TimestampRule, ValidationIssue,
};

#[cfg(target_os = "linux")]
pub use safe_fs::{FileIdentity, SafeFile, SafeOpenError, SafeRoot};
#[cfg(target_os = "linux")]
pub use source_discovery::{
    MAX_DISCOVERY_DIRECTORIES, MAX_DISCOVERY_ENTRIES, SourceDiscoveryError, discover_regular_files,
};
#[cfg(target_os = "linux")]
pub use source_registry::{
    ConfiguredFile, ConfiguredLogSource, LogSourceInfo, SourceRegistry, SourceRegistryError,
};
