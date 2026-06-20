#![forbid(unsafe_code)]

pub mod config;

pub use config::{
    AppConfig, ConfigLoadError, ConfigValidationError, DirectoryRule, Encoding, LimitsConfig,
    LogSourceConfig, TimestampRule, ValidationIssue,
};
