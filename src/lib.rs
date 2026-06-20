#![forbid(unsafe_code)]

pub mod config;

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
