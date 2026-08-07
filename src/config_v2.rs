use std::{
    collections::HashSet,
    fmt,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AppConfig, CONFIG_VERSION, ConfigValidationError, DirectoryRule, Encoding, LimitsConfigV2,
    LogSourceConfig, TimestampRule, config_limits_v2::LimitsConfigV2ValidationError,
};

pub const CONFIG_VERSION_V2: u32 = 2;
const MAX_CONNECTIONS: usize = 100;
const MAX_IDENTIFIER_CHARS: usize = 128;
const MAX_HOST_CHARS: usize = 255;
const MAX_USERNAME_CHARS: usize = 128;
const MAX_SECRET_REF_CHARS: usize = 256;
const MAX_PATH_CHARS: usize = 4096;
const MAX_CACHE_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_CACHE_BYTES_PER_SOURCE: u64 = 256 * 1024 * 1024 * 1024;
const MIN_CACHE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfigV2 {
    pub version: u32,
    #[serde(default)]
    pub connections: Vec<SshConnectionConfig>,
    pub sources: Vec<LogSourceConfigV2>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
    #[serde(default)]
    pub limits: LimitsConfigV2,
}

impl AppConfigV2 {
    pub fn from_json_str(input: &str) -> Result<Self, ConfigV2LoadError> {
        let config: Self = serde_json::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), ConfigV2ValidationError> {
        let mut issues = Vec::new();

        if self.version != CONFIG_VERSION_V2 {
            push_issue(
                &mut issues,
                "version",
                format!("only configuration version {CONFIG_VERSION_V2} is supported"),
            );
        }

        if let Err(error) = self.as_v1_shape().validate() {
            append_v1_issues(&mut issues, &error);
        }
        if let Err(error) = self.limits.validate_remote() {
            let (field, message) = match error {
                LimitsConfigV2ValidationError::ConcurrentSshConnections => (
                    "limits.max_concurrent_ssh_connections",
                    "must be between 1 and 64",
                ),
                LimitsConfigV2ValidationError::SyncBytesPerQuery => (
                    "limits.max_sync_bytes_per_query",
                    "must be between 1 and 64 GiB",
                ),
                LimitsConfigV2ValidationError::RemoteFilesPerSource => (
                    "limits.max_remote_files_per_source",
                    "must be between 1 and 10000",
                ),
            };
            push_issue(&mut issues, field, message);
        }

        if self.connections.len() > MAX_CONNECTIONS {
            push_issue(
                &mut issues,
                "connections",
                format!("must not contain more than {MAX_CONNECTIONS} values"),
            );
        }

        let mut connection_ids = HashSet::with_capacity(self.connections.len());
        for (index, connection) in self.connections.iter().enumerate() {
            let prefix = format!("connections[{index}]");
            connection.validate(&prefix, &mut issues);
            if !connection_ids.insert(connection.connection_id.as_str()) {
                push_issue(
                    &mut issues,
                    format!("{prefix}.connection_id"),
                    "must be globally unique",
                );
            }
        }

        let mut has_remote = false;
        for (index, source) in self.sources.iter().enumerate() {
            let prefix = format!("sources[{index}]");
            match source.backend.backend_type {
                BackendType::Local => {
                    if source.backend.connection_id.is_some() {
                        push_issue(
                            &mut issues,
                            format!("{prefix}.backend.connection_id"),
                            "must not be set for a local backend",
                        );
                    }
                    if source.sync.is_some() {
                        push_issue(
                            &mut issues,
                            format!("{prefix}.sync"),
                            "must not be set for a local backend",
                        );
                    }
                }
                BackendType::Ssh => {
                    has_remote = true;
                    let Some(connection_id) = source.backend.connection_id.as_deref() else {
                        push_issue(
                            &mut issues,
                            format!("{prefix}.backend.connection_id"),
                            "is required for an ssh backend",
                        );
                        continue;
                    };
                    if !connection_ids.contains(connection_id) {
                        push_issue(
                            &mut issues,
                            format!("{prefix}.backend.connection_id"),
                            "must reference a configured connection",
                        );
                    }
                    match source.sync.as_ref() {
                        Some(sync) => sync.validate(&format!("{prefix}.sync"), &mut issues),
                        None => push_issue(
                            &mut issues,
                            format!("{prefix}.sync"),
                            "is required for an ssh backend",
                        ),
                    }
                }
            }
        }

        if has_remote && self.connections.is_empty() {
            push_issue(
                &mut issues,
                "connections",
                "is required when an ssh source is configured",
            );
        }
        if has_remote && self.cache.is_none() {
            push_issue(
                &mut issues,
                "cache",
                "is required when an ssh source is configured",
            );
        }

        if let Some(cache) = &self.cache {
            cache.validate("cache", &mut issues);
        }

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigV2ValidationError { issues })
        }
    }

    #[must_use]
    pub fn as_v1_shape(&self) -> AppConfig {
        AppConfig {
            version: CONFIG_VERSION,
            sources: self
                .sources
                .iter()
                .map(LogSourceConfigV2::to_v1_config)
                .collect(),
            limits: self.limits.local_limits(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshConnectionConfig {
    pub connection_id: String,
    #[serde(rename = "type")]
    pub connection_type: ConnectionType,
    pub host: String,
    #[serde(default = "default_ssh_port")]
    pub port: u16,
    pub username: String,
    pub auth: SshAuthConfig,
    pub host_key: HostKeyConfig,
    #[serde(default = "default_connect_timeout_millis")]
    pub connect_timeout_millis: u64,
    #[serde(default = "default_operation_timeout_millis")]
    pub operation_timeout_millis: u64,
    #[serde(default = "default_keepalive_seconds")]
    pub keepalive_seconds: Option<u64>,
}

impl SshConnectionConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        if !is_identifier(&self.connection_id) {
            push_issue(
                issues,
                format!("{prefix}.connection_id"),
                "must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}",
            );
        }
        if self.host.is_empty()
            || self.host.len() > MAX_HOST_CHARS
            || self.host.chars().any(char::is_whitespace)
            || self.host.contains('/')
        {
            push_issue(
                issues,
                format!("{prefix}.host"),
                "must be a non-empty host name or address without whitespace or '/'",
            );
        }
        if self.port == 0 {
            push_issue(
                issues,
                format!("{prefix}.port"),
                "must be between 1 and 65535",
            );
        }
        if self.username.is_empty()
            || self.username.len() > MAX_USERNAME_CHARS
            || self.username.chars().any(char::is_whitespace)
            || self.username.chars().any(char::is_control)
        {
            push_issue(
                issues,
                format!("{prefix}.username"),
                "must be a non-empty SSH username without whitespace or control characters",
            );
        }
        if !(100..=60_000).contains(&self.connect_timeout_millis) {
            push_issue(
                issues,
                format!("{prefix}.connect_timeout_millis"),
                "must be between 100 and 60000",
            );
        }
        if !(100..=600_000).contains(&self.operation_timeout_millis) {
            push_issue(
                issues,
                format!("{prefix}.operation_timeout_millis"),
                "must be between 100 and 600000",
            );
        }
        if self
            .keepalive_seconds
            .is_some_and(|value| !(5..=3600).contains(&value))
        {
            push_issue(
                issues,
                format!("{prefix}.keepalive_seconds"),
                "must be null or between 5 and 3600",
            );
        }
        self.auth.validate(&format!("{prefix}.auth"), issues);
        self.host_key
            .validate(&format!("{prefix}.host_key"), issues);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ConnectionType {
    #[serde(rename = "ssh")]
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SshAuthConfig {
    #[serde(rename = "type")]
    pub auth_type: SshAuthType,
    #[serde(default)]
    pub secret_ref: Option<String>,
    #[serde(default)]
    pub key_file: Option<PathBuf>,
    #[serde(default)]
    pub passphrase_secret_ref: Option<String>,
}

impl SshAuthConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        match self.auth_type {
            SshAuthType::Password => {
                validate_secret_ref(
                    self.secret_ref.as_deref(),
                    &format!("{prefix}.secret_ref"),
                    true,
                    issues,
                );
                if self.key_file.is_some() || self.passphrase_secret_ref.is_some() {
                    push_issue(
                        issues,
                        prefix,
                        "password auth must not contain key_file or passphrase_secret_ref",
                    );
                }
            }
            SshAuthType::PrivateKey => {
                if self.secret_ref.is_some() {
                    push_issue(
                        issues,
                        format!("{prefix}.secret_ref"),
                        "must not be set for private_key auth",
                    );
                }
                match self.key_file.as_ref() {
                    Some(path) if valid_local_path(path) => {}
                    _ => push_issue(
                        issues,
                        format!("{prefix}.key_file"),
                        "is required and must be a non-empty local path",
                    ),
                }
                validate_secret_ref(
                    self.passphrase_secret_ref.as_deref(),
                    &format!("{prefix}.passphrase_secret_ref"),
                    false,
                    issues,
                );
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum SshAuthType {
    #[serde(rename = "password")]
    Password,
    #[serde(rename = "private_key")]
    PrivateKey,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HostKeyConfig {
    pub known_hosts_file: PathBuf,
}

impl HostKeyConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        if !valid_local_path(&self.known_hosts_file) {
            push_issue(
                issues,
                format!("{prefix}.known_hosts_file"),
                "must be a non-empty local path",
            );
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogSourceConfigV2 {
    pub source_id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub service: String,
    pub environment: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub encoding: Encoding,
    pub backend: SourceBackendConfig,
    pub root: PathBuf,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub directories: Vec<DirectoryRule>,
    #[serde(default)]
    pub timestamp_rule: Option<TimestampRule>,
    #[serde(default)]
    pub sync: Option<RemoteSyncPolicy>,
}

impl LogSourceConfigV2 {
    #[must_use]
    pub fn to_v1_config(&self) -> LogSourceConfig {
        LogSourceConfig {
            source_id: self.source_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            service: self.service.clone(),
            environment: self.environment.clone(),
            tags: self.tags.clone(),
            enabled: self.enabled,
            encoding: self.encoding,
            root: self.root.clone(),
            files: self.files.clone(),
            directories: self.directories.clone(),
            timestamp_rule: self.timestamp_rule.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SourceBackendConfig {
    #[serde(rename = "type")]
    pub backend_type: BackendType,
    #[serde(default)]
    pub connection_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BackendType {
    #[serde(rename = "local")]
    Local,
    #[serde(rename = "ssh")]
    Ssh,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RemoteSyncPolicy {
    pub freshness: FreshnessPolicy,
    pub bootstrap: BootstrapPolicy,
    #[serde(default)]
    pub allow_stale_on_error: bool,
}

impl RemoteSyncPolicy {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        if self.allow_stale_on_error {
            push_issue(
                issues,
                format!("{prefix}.allow_stale_on_error"),
                "must remain false in the v2 MVP",
            );
        }
        self.bootstrap
            .validate(&format!("{prefix}.bootstrap"), issues);
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum FreshnessPolicy {
    #[serde(rename = "on_query")]
    OnQuery,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootstrapPolicy {
    #[serde(rename = "type")]
    pub bootstrap_type: BootstrapType,
    #[serde(default)]
    pub bytes: Option<u64>,
}

impl BootstrapPolicy {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        match self.bootstrap_type {
            BootstrapType::Full | BootstrapType::FromNow => {
                if self.bytes.is_some() {
                    push_issue(
                        issues,
                        format!("{prefix}.bytes"),
                        "must not be set for this bootstrap type",
                    );
                }
            }
            BootstrapType::Tail => match self.bytes {
                Some(value) if value > 0 && value <= 64 * 1024 * 1024 * 1024 => {}
                _ => push_issue(
                    issues,
                    format!("{prefix}.bytes"),
                    "is required for tail and must be between 1 and 64 GiB",
                ),
            },
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BootstrapType {
    #[serde(rename = "full")]
    Full,
    #[serde(rename = "tail")]
    Tail,
    #[serde(rename = "from_now")]
    FromNow,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CacheConfig {
    pub root: PathBuf,
    pub max_bytes: u64,
    pub max_bytes_per_source: u64,
    pub retention_hours: u64,
    pub max_generations_per_file: usize,
}

impl CacheConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ConfigV2ValidationIssue>) {
        if !self.root.is_absolute() || !valid_local_path(&self.root) {
            push_issue(
                issues,
                format!("{prefix}.root"),
                "must be a non-empty absolute path",
            );
        }
        if !(MIN_CACHE_BYTES..=MAX_CACHE_BYTES).contains(&self.max_bytes) {
            push_issue(
                issues,
                format!("{prefix}.max_bytes"),
                "must be between 1 MiB and 1 TiB",
            );
        }
        if !(MIN_CACHE_BYTES..=MAX_CACHE_BYTES_PER_SOURCE).contains(&self.max_bytes_per_source) {
            push_issue(
                issues,
                format!("{prefix}.max_bytes_per_source"),
                "must be between 1 MiB and 256 GiB",
            );
        }
        if self.max_bytes_per_source > self.max_bytes {
            push_issue(
                issues,
                format!("{prefix}.max_bytes_per_source"),
                "must not exceed max_bytes",
            );
        }
        if !(1..=8760).contains(&self.retention_hours) {
            push_issue(
                issues,
                format!("{prefix}.retention_hours"),
                "must be between 1 and 8760",
            );
        }
        if !(2..=32).contains(&self.max_generations_per_file) {
            push_issue(
                issues,
                format!("{prefix}.max_generations_per_file"),
                "must be between 2 and 32",
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigV2ValidationIssue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigV2ValidationError {
    issues: Vec<ConfigV2ValidationIssue>,
}

impl ConfigV2ValidationError {
    #[must_use]
    pub fn issues(&self) -> &[ConfigV2ValidationIssue] {
        &self.issues
    }
}

impl fmt::Display for ConfigV2ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "v2 configuration validation failed")?;
        for issue in &self.issues {
            write!(formatter, "; {}: {}", issue.field, issue.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigV2ValidationError {}

#[derive(Debug, Error)]
pub enum ConfigV2LoadError {
    #[error("failed to parse v2 configuration")]
    Parse(#[from] serde_json::Error),

    #[error(transparent)]
    Validation(#[from] ConfigV2ValidationError),
}

fn append_v1_issues(issues: &mut Vec<ConfigV2ValidationIssue>, error: &ConfigValidationError) {
    for issue in error.issues() {
        push_issue(issues, issue.field.clone(), issue.message.clone());
    }
}

fn push_issue(
    issues: &mut Vec<ConfigV2ValidationIssue>,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    issues.push(ConfigV2ValidationIssue {
        field: field.into(),
        message: message.into(),
    });
}

fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_alphanumeric() {
        return false;
    }
    let mut count = 1;
    for character in chars {
        count += 1;
        if count > MAX_IDENTIFIER_CHARS
            || !(character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-'))
        {
            return false;
        }
    }
    true
}

fn validate_secret_ref(
    value: Option<&str>,
    field: &str,
    required: bool,
    issues: &mut Vec<ConfigV2ValidationIssue>,
) {
    match value {
        Some(value)
            if !value.is_empty()
                && value.len() <= MAX_SECRET_REF_CHARS
                && value.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "._:/-".contains(character)
                }) => {}
        None if !required => {}
        _ => push_issue(
            issues,
            field,
            "must be a valid secret reference containing only [A-Za-z0-9._:/-]",
        ),
    }
}

fn valid_local_path(path: &Path) -> bool {
    let rendered = path.to_string_lossy();
    !rendered.is_empty() && rendered.len() <= MAX_PATH_CHARS && !rendered.contains('\0')
}

const fn default_ssh_port() -> u16 {
    22
}

const fn default_connect_timeout_millis() -> u64 {
    5_000
}

const fn default_operation_timeout_millis() -> u64 {
    30_000
}

const fn default_keepalive_seconds() -> Option<u64> {
    Some(30)
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID_LOCAL: &str = include_str!("../tests/contracts/v2/valid/local-only.json");
    const VALID_SSH_PASSWORD: &str =
        include_str!("../tests/contracts/v2/valid/ssh-password-tail.json");
    const INVALID_UNKNOWN_CONNECTION: &str =
        include_str!("../tests/contracts/v2/invalid/unknown-connection.json");

    #[test]
    fn parses_contract_local_fixture() {
        assert!(AppConfigV2::from_json_str(VALID_LOCAL).is_ok());
    }

    #[test]
    fn parses_contract_ssh_fixture() {
        assert!(AppConfigV2::from_json_str(VALID_SSH_PASSWORD).is_ok());
    }

    #[test]
    fn rejects_contract_unknown_connection_fixture() {
        assert!(AppConfigV2::from_json_str(INVALID_UNKNOWN_CONNECTION).is_err());
    }

    #[test]
    fn parses_remote_specific_limits() {
        let input = r#"{
            "version": 2,
            "sources": [{
                "source_id": "local",
                "name": "local",
                "service": "local",
                "environment": "test",
                "backend": {"type": "local"},
                "root": "/var/log/local",
                "files": ["application.log"]
            }],
            "limits": {
                "max_concurrent_ssh_connections": 8,
                "max_sync_bytes_per_query": 1048576,
                "max_remote_files_per_source": 100
            }
        }"#;

        let config = AppConfigV2::from_json_str(input).expect("v2 limits should parse");
        assert_eq!(config.limits.max_concurrent_ssh_connections, 8);
        assert_eq!(config.limits.max_sync_bytes_per_query, 1_048_576);
        assert_eq!(config.limits.max_remote_files_per_source, 100);
    }
}
