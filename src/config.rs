use std::{
    collections::HashSet,
    fmt,
    fs,
    path::{Path, PathBuf},
};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const CONFIG_VERSION: u32 = 1;
const MAX_CONFIGURED_SOURCES: usize = 100;
const MAX_SOURCE_ID_CHARS: usize = 128;
const MAX_NAME_CHARS: usize = 256;
const MAX_DESCRIPTION_CHARS: usize = 1024;
const MAX_SERVICE_CHARS: usize = 256;
const MAX_ENVIRONMENT_CHARS: usize = 128;
const MAX_TAGS: usize = 32;
const MAX_TAG_CHARS: usize = 64;
const MAX_FILES_PER_SOURCE: usize = 10_000;
const MAX_DIRECTORIES_PER_SOURCE: usize = 64;
const MAX_SUFFIXES_PER_DIRECTORY: usize = 32;
const MAX_SUFFIX_CHARS: usize = 128;
const MAX_PATH_CHARS: usize = 4096;
const MAX_TIMESTAMP_PREFIX_BYTES: usize = 256;
const MAX_TIMESTAMP_FORMAT_CHARS: usize = 128;

const HARD_MAX_SOURCES_PER_QUERY: usize = 10;
const HARD_MAX_SCAN_FILES_PER_QUERY: usize = 10_000;
const HARD_MAX_SCAN_BYTES_PER_PAGE: u64 = 64 * 1024 * 1024 * 1024;
const HARD_MAX_QUERY_TIMEOUT_MILLIS: u64 = 10 * 60 * 1000;
const HARD_MAX_RESULTS_PER_PAGE: usize = 200;
const HARD_MAX_LINE_BYTES: usize = 1024 * 1024;
const HARD_MAX_RETURNED_CONTENT_BYTES: usize = 16 * 1024 * 1024;
const HARD_MAX_RESPONSE_BYTES: usize = 64 * 1024 * 1024;
const HARD_MAX_CONTEXT_LINES_PER_SIDE: usize = 50;
const HARD_MAX_CONCURRENT_SCANS: usize = 64;
const HARD_MAX_STATE_CAPACITY: usize = 1_000_000;
const HARD_MAX_STATE_TTL_SECONDS: u64 = 24 * 60 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AppConfig {
    pub version: u32,
    pub sources: Vec<LogSourceConfig>,
    #[serde(default)]
    pub limits: LimitsConfig,
}

impl AppConfig {
    pub fn from_json_str(input: &str) -> Result<Self, ConfigLoadError> {
        let config: Self = serde_json::from_str(input)?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigLoadError> {
        let input = fs::read_to_string(path).map_err(ConfigLoadError::Read)?;
        Self::from_json_str(&input)
    }

    pub fn validate(&self) -> Result<(), ConfigValidationError> {
        let mut issues = Vec::new();

        if self.version != CONFIG_VERSION {
            push_issue(
                &mut issues,
                "version",
                format!("only configuration version {CONFIG_VERSION} is supported"),
            );
        }

        if self.sources.is_empty() || self.sources.len() > MAX_CONFIGURED_SOURCES {
            push_issue(
                &mut issues,
                "sources",
                format!(
                    "must contain between 1 and {MAX_CONFIGURED_SOURCES} log sources"
                ),
            );
        }

        let mut source_ids = HashSet::with_capacity(self.sources.len());
        for (index, source) in self.sources.iter().enumerate() {
            let prefix = format!("sources[{index}]");
            source.validate(&prefix, &mut issues);
            if !source_ids.insert(source.source_id.as_str()) {
                push_issue(
                    &mut issues,
                    format!("{prefix}.source_id"),
                    "must be globally unique",
                );
            }
        }

        self.limits.validate("limits", &mut issues);

        if issues.is_empty() {
            Ok(())
        } else {
            Err(ConfigValidationError { issues })
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LogSourceConfig {
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
    pub root: PathBuf,
    #[serde(default)]
    pub files: Vec<PathBuf>,
    #[serde(default)]
    pub directories: Vec<DirectoryRule>,
    #[serde(default)]
    pub timestamp_rule: Option<TimestampRule>,
}

impl LogSourceConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ValidationIssue>) {
        if !is_valid_source_id(&self.source_id) {
            push_issue(
                issues,
                format!("{prefix}.source_id"),
                "must match [A-Za-z0-9][A-Za-z0-9._-]{0,127}",
            );
        }
        validate_non_empty_text(
            &self.name,
            MAX_NAME_CHARS,
            format!("{prefix}.name"),
            issues,
        );
        validate_optional_text(
            &self.description,
            MAX_DESCRIPTION_CHARS,
            format!("{prefix}.description"),
            issues,
        );
        validate_non_empty_text(
            &self.service,
            MAX_SERVICE_CHARS,
            format!("{prefix}.service"),
            issues,
        );
        validate_non_empty_text(
            &self.environment,
            MAX_ENVIRONMENT_CHARS,
            format!("{prefix}.environment"),
            issues,
        );

        if self.tags.len() > MAX_TAGS {
            push_issue(
                issues,
                format!("{prefix}.tags"),
                format!("must not contain more than {MAX_TAGS} values"),
            );
        }
        let mut tags = HashSet::with_capacity(self.tags.len());
        for (index, tag) in self.tags.iter().enumerate() {
            validate_non_empty_text(
                tag,
                MAX_TAG_CHARS,
                format!("{prefix}.tags[{index}]"),
                issues,
            );
            if !tags.insert(tag.as_str()) {
                push_issue(
                    issues,
                    format!("{prefix}.tags[{index}]"),
                    "must be unique within the source",
                );
            }
        }

        if !is_valid_root_path(&self.root) {
            push_issue(
                issues,
                format!("{prefix}.root"),
                "must be a non-empty Linux absolute path without NUL bytes",
            );
        }

        if self.files.is_empty() && self.directories.is_empty() {
            push_issue(
                issues,
                prefix,
                "must contain at least one explicit file or directory rule",
            );
        }

        if self.files.len() > MAX_FILES_PER_SOURCE {
            push_issue(
                issues,
                format!("{prefix}.files"),
                format!("must not contain more than {MAX_FILES_PER_SOURCE} paths"),
            );
        }
        let mut files = HashSet::with_capacity(self.files.len());
        for (index, path) in self.files.iter().enumerate() {
            if !is_normal_relative_path(path, false) {
                push_issue(
                    issues,
                    format!("{prefix}.files[{index}]"),
                    "must be a normalized relative path without '.' or '..' components",
                );
            }
            if !files.insert(path) {
                push_issue(
                    issues,
                    format!("{prefix}.files[{index}]"),
                    "must be unique within the source",
                );
            }
        }

        if self.directories.len() > MAX_DIRECTORIES_PER_SOURCE {
            push_issue(
                issues,
                format!("{prefix}.directories"),
                format!(
                    "must not contain more than {MAX_DIRECTORIES_PER_SOURCE} directory rules"
                ),
            );
        }
        let mut directories = HashSet::with_capacity(self.directories.len());
        for (index, directory) in self.directories.iter().enumerate() {
            let directory_prefix = format!("{prefix}.directories[{index}]");
            directory.validate(&directory_prefix, issues);
            if !directories.insert(directory.path.as_path()) {
                push_issue(
                    issues,
                    format!("{directory_prefix}.path"),
                    "must be unique within the source",
                );
            }
        }

        if let Some(rule) = &self.timestamp_rule {
            rule.validate(&format!("{prefix}.timestamp_rule"), issues);
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    #[serde(rename = "utf-8")]
    Utf8,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DirectoryRule {
    pub path: PathBuf,
    #[serde(default)]
    pub recursive: bool,
    pub include_suffixes: Vec<String>,
}

impl DirectoryRule {
    fn validate(&self, prefix: &str, issues: &mut Vec<ValidationIssue>) {
        if !is_normal_relative_path(&self.path, true) {
            push_issue(
                issues,
                format!("{prefix}.path"),
                "must be '.' or a normalized relative path without '.' or '..' components",
            );
        }

        if self.include_suffixes.is_empty()
            || self.include_suffixes.len() > MAX_SUFFIXES_PER_DIRECTORY
        {
            push_issue(
                issues,
                format!("{prefix}.include_suffixes"),
                format!(
                    "must contain between 1 and {MAX_SUFFIXES_PER_DIRECTORY} suffixes"
                ),
            );
        }

        let mut suffixes = HashSet::with_capacity(self.include_suffixes.len());
        for (index, suffix) in self.include_suffixes.iter().enumerate() {
            if !is_valid_suffix(suffix) {
                push_issue(
                    issues,
                    format!("{prefix}.include_suffixes[{index}]"),
                    "must start with '.', contain no path separators or line breaks, and be at most 128 characters",
                );
            }
            if !suffixes.insert(suffix.as_str()) {
                push_issue(
                    issues,
                    format!("{prefix}.include_suffixes[{index}]"),
                    "must be unique within the directory rule",
                );
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimestampRule {
    Rfc3339 {
        prefix_bytes: usize,
    },
    Custom {
        prefix_bytes: usize,
        format: String,
        #[serde(default)]
        default_offset_seconds: Option<i32>,
    },
}

impl TimestampRule {
    fn validate(&self, prefix: &str, issues: &mut Vec<ValidationIssue>) {
        let prefix_bytes = match self {
            Self::Rfc3339 { prefix_bytes } | Self::Custom { prefix_bytes, .. } => *prefix_bytes,
        };
        if prefix_bytes == 0 || prefix_bytes > MAX_TIMESTAMP_PREFIX_BYTES {
            push_issue(
                issues,
                format!("{prefix}.prefix_bytes"),
                format!(
                    "must be between 1 and {MAX_TIMESTAMP_PREFIX_BYTES} bytes"
                ),
            );
        }

        if let Self::Custom {
            format,
            default_offset_seconds,
            ..
        } = self
        {
            validate_non_empty_text(
                format,
                MAX_TIMESTAMP_FORMAT_CHARS,
                format!("{prefix}.format"),
                issues,
            );
            if default_offset_seconds
                .is_some_and(|seconds| !(-86_399..=86_399).contains(&seconds))
            {
                push_issue(
                    issues,
                    format!("{prefix}.default_offset_seconds"),
                    "must be between -86399 and 86399",
                );
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    pub max_sources_per_query: usize,
    pub max_scan_files_per_query: usize,
    pub max_scan_bytes_per_page: u64,
    pub query_timeout_millis: u64,
    pub default_results_per_page: usize,
    pub max_results_per_page: usize,
    pub max_line_bytes: usize,
    pub max_returned_content_bytes: usize,
    pub max_response_bytes: usize,
    pub max_context_lines_per_side: usize,
    pub max_concurrent_scans: usize,
    pub match_reference_capacity: usize,
    pub match_reference_ttl_seconds: u64,
    pub cursor_capacity: usize,
    pub cursor_ttl_seconds: u64,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        Self {
            max_sources_per_query: 10,
            max_scan_files_per_query: 500,
            max_scan_bytes_per_page: 512 * 1024 * 1024,
            query_timeout_millis: 10_000,
            default_results_per_page: 50,
            max_results_per_page: 200,
            max_line_bytes: 16 * 1024,
            max_returned_content_bytes: 512 * 1024,
            max_response_bytes: 1024 * 1024,
            max_context_lines_per_side: 50,
            max_concurrent_scans: 4,
            match_reference_capacity: 10_000,
            match_reference_ttl_seconds: 600,
            cursor_capacity: 1_000,
            cursor_ttl_seconds: 300,
        }
    }
}

impl LimitsConfig {
    fn validate(&self, prefix: &str, issues: &mut Vec<ValidationIssue>) {
        validate_bounded_usize(
            self.max_sources_per_query,
            HARD_MAX_SOURCES_PER_QUERY,
            format!("{prefix}.max_sources_per_query"),
            issues,
        );
        validate_bounded_usize(
            self.max_scan_files_per_query,
            HARD_MAX_SCAN_FILES_PER_QUERY,
            format!("{prefix}.max_scan_files_per_query"),
            issues,
        );
        validate_bounded_u64(
            self.max_scan_bytes_per_page,
            HARD_MAX_SCAN_BYTES_PER_PAGE,
            format!("{prefix}.max_scan_bytes_per_page"),
            issues,
        );
        validate_bounded_u64(
            self.query_timeout_millis,
            HARD_MAX_QUERY_TIMEOUT_MILLIS,
            format!("{prefix}.query_timeout_millis"),
            issues,
        );
        validate_bounded_usize(
            self.default_results_per_page,
            HARD_MAX_RESULTS_PER_PAGE,
            format!("{prefix}.default_results_per_page"),
            issues,
        );
        validate_bounded_usize(
            self.max_results_per_page,
            HARD_MAX_RESULTS_PER_PAGE,
            format!("{prefix}.max_results_per_page"),
            issues,
        );
        validate_bounded_usize(
            self.max_line_bytes,
            HARD_MAX_LINE_BYTES,
            format!("{prefix}.max_line_bytes"),
            issues,
        );
        validate_bounded_usize(
            self.max_returned_content_bytes,
            HARD_MAX_RETURNED_CONTENT_BYTES,
            format!("{prefix}.max_returned_content_bytes"),
            issues,
        );
        validate_bounded_usize(
            self.max_response_bytes,
            HARD_MAX_RESPONSE_BYTES,
            format!("{prefix}.max_response_bytes"),
            issues,
        );
        validate_bounded_usize(
            self.max_context_lines_per_side,
            HARD_MAX_CONTEXT_LINES_PER_SIDE,
            format!("{prefix}.max_context_lines_per_side"),
            issues,
        );
        validate_bounded_usize(
            self.max_concurrent_scans,
            HARD_MAX_CONCURRENT_SCANS,
            format!("{prefix}.max_concurrent_scans"),
            issues,
        );
        validate_bounded_usize(
            self.match_reference_capacity,
            HARD_MAX_STATE_CAPACITY,
            format!("{prefix}.match_reference_capacity"),
            issues,
        );
        validate_bounded_u64(
            self.match_reference_ttl_seconds,
            HARD_MAX_STATE_TTL_SECONDS,
            format!("{prefix}.match_reference_ttl_seconds"),
            issues,
        );
        validate_bounded_usize(
            self.cursor_capacity,
            HARD_MAX_STATE_CAPACITY,
            format!("{prefix}.cursor_capacity"),
            issues,
        );
        validate_bounded_u64(
            self.cursor_ttl_seconds,
            HARD_MAX_STATE_TTL_SECONDS,
            format!("{prefix}.cursor_ttl_seconds"),
            issues,
        );

        if self.default_results_per_page > self.max_results_per_page {
            push_issue(
                issues,
                format!("{prefix}.default_results_per_page"),
                "must not exceed max_results_per_page",
            );
        }
        if self.max_line_bytes > self.max_returned_content_bytes {
            push_issue(
                issues,
                format!("{prefix}.max_line_bytes"),
                "must not exceed max_returned_content_bytes",
            );
        }
        if self.max_returned_content_bytes >= self.max_response_bytes {
            push_issue(
                issues,
                format!("{prefix}.max_returned_content_bytes"),
                "must be smaller than max_response_bytes",
            );
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationIssue {
    pub field: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigValidationError {
    issues: Vec<ValidationIssue>,
}

impl ConfigValidationError {
    #[must_use]
    pub fn issues(&self) -> &[ValidationIssue] {
        &self.issues
    }
}

impl fmt::Display for ConfigValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "configuration validation failed")?;
        for issue in &self.issues {
            write!(formatter, "; {}: {}", issue.field, issue.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ConfigValidationError {}

#[derive(Debug, Error)]
pub enum ConfigLoadError {
    #[error("failed to read configuration")]
    Read(#[source] std::io::Error),

    #[error("failed to parse configuration")]
    Parse(#[from] serde_json::Error),

    #[error(transparent)]
    Validation(#[from] ConfigValidationError),
}

const fn default_true() -> bool {
    true
}

fn push_issue(
    issues: &mut Vec<ValidationIssue>,
    field: impl Into<String>,
    message: impl Into<String>,
) {
    issues.push(ValidationIssue {
        field: field.into(),
        message: message.into(),
    });
}

fn validate_non_empty_text(
    value: &str,
    max_chars: usize,
    field: impl Into<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    let field = field.into();
    let count = value.chars().count();
    if count == 0 || count > max_chars || value.contains('\0') {
        push_issue(
            issues,
            field,
            format!("must contain between 1 and {max_chars} characters and no NUL bytes"),
        );
    }
}

fn validate_optional_text(
    value: &str,
    max_chars: usize,
    field: impl Into<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    if value.chars().count() > max_chars || value.contains('\0') {
        push_issue(
            issues,
            field,
            format!("must contain at most {max_chars} characters and no NUL bytes"),
        );
    }
}

fn validate_bounded_usize(
    value: usize,
    maximum: usize,
    field: impl Into<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    if value == 0 || value > maximum {
        push_issue(
            issues,
            field,
            format!("must be between 1 and {maximum}"),
        );
    }
}

fn validate_bounded_u64(
    value: u64,
    maximum: u64,
    field: impl Into<String>,
    issues: &mut Vec<ValidationIssue>,
) {
    if value == 0 || value > maximum {
        push_issue(
            issues,
            field,
            format!("must be between 1 and {maximum}"),
        );
    }
}

fn is_valid_source_id(value: &str) -> bool {
    let count = value.chars().count();
    if count == 0 || count > MAX_SOURCE_ID_CHARS || !value.is_ascii() {
        return false;
    }

    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn is_valid_root_path(path: &Path) -> bool {
    let Some(raw) = path.to_str() else {
        return false;
    };
    path.is_absolute()
        && !raw.is_empty()
        && raw.chars().count() <= MAX_PATH_CHARS
        && !raw.contains('\0')
}

fn is_normal_relative_path(path: &Path, allow_single_dot: bool) -> bool {
    let Some(raw) = path.to_str() else {
        return false;
    };
    if allow_single_dot && raw == "." {
        return true;
    }
    if raw.is_empty()
        || raw.starts_with('/')
        || raw.ends_with('/')
        || raw.contains("//")
        || raw.contains('\0')
        || raw.chars().count() > MAX_PATH_CHARS
    {
        return false;
    }

    raw.split('/')
        .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn is_valid_suffix(value: &str) -> bool {
    let count = value.chars().count();
    count > 0
        && count <= MAX_SUFFIX_CHARS
        && value.starts_with('.')
        && !value.contains('/')
        && !value.contains('\0')
        && !value.contains('\r')
        && !value.contains('\n')
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    const EXAMPLE: &str = include_str!("../examples/log-query-mcp.v1.json");

    #[test]
    fn accepts_frozen_v1_example() {
        let config = AppConfig::from_json_str(EXAMPLE).expect("example should be valid");

        assert_eq!(config.version, CONFIG_VERSION);
        assert_eq!(config.sources.len(), 2);
        assert_eq!(config.limits, LimitsConfig::default());
    }

    #[test]
    fn loads_configuration_from_file() {
        let mut file = NamedTempFile::new().expect("temporary file should be created");
        file.write_all(EXAMPLE.as_bytes())
            .expect("example should be written");

        let config = AppConfig::load(file.path()).expect("configuration should load");
        assert_eq!(config.sources[0].source_id, "payment-test");
    }

    #[test]
    fn applies_frozen_defaults() {
        let input = r#"
        {
          "version": 1,
          "sources": [{
            "source_id": "payment-test",
            "name": "Payment",
            "service": "payment-service",
            "environment": "test",
            "root": "/var/log/payment",
            "files": ["application.log"]
          }]
        }
        "#;

        let config = AppConfig::from_json_str(input).expect("minimal configuration should work");
        let source = &config.sources[0];
        assert!(source.enabled);
        assert_eq!(source.encoding, Encoding::Utf8);
        assert!(source.description.is_empty());
        assert!(source.tags.is_empty());
        assert_eq!(config.limits, LimitsConfig::default());
    }

    #[test]
    fn rejects_unknown_json_fields() {
        let input = r#"
        {
          "version": 1,
          "unexpected": true,
          "sources": []
        }
        "#;

        assert!(matches!(
            AppConfig::from_json_str(input),
            Err(ConfigLoadError::Parse(_))
        ));
    }

    #[test]
    fn rejects_unsupported_version_and_duplicate_source_ids() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.version = 2;
        config.sources[1].source_id = config.sources[0].source_id.clone();

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(&error, "version"));
        assert!(has_issue(&error, "sources[1].source_id"));
    }

    #[test]
    fn rejects_unsafe_or_duplicate_paths() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.sources[0].files = vec![
            PathBuf::from("../secret.log"),
            PathBuf::from("application.log"),
            PathBuf::from("application.log"),
        ];
        config.sources[0].directories[0].path = PathBuf::from("archive/../secret");

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(&error, "sources[0].files[0]"));
        assert!(has_issue(&error, "sources[0].files[2]"));
        assert!(has_issue(&error, "sources[0].directories[0].path"));
    }

    #[test]
    fn accepts_root_directory_rule_dot() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.sources[0].files.clear();
        config.sources[0].directories = vec![DirectoryRule {
            path: PathBuf::from("."),
            recursive: false,
            include_suffixes: vec![".log".to_owned()],
        }];

        config.validate().expect("dot directory rule should be valid");
    }

    #[test]
    fn rejects_source_without_files_or_directories() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.sources[0].files.clear();
        config.sources[0].directories.clear();

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(&error, "sources[0]"));
    }

    #[test]
    fn rejects_duplicate_tags_and_suffixes() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.sources[0].tags = vec!["java".to_owned(), "java".to_owned()];
        config.sources[0].directories[0].include_suffixes =
            vec![".log".to_owned(), ".log".to_owned()];

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(&error, "sources[0].tags[1]"));
        assert!(has_issue(
            &error,
            "sources[0].directories[0].include_suffixes[1]"
        ));
    }

    #[test]
    fn rejects_invalid_limit_relationships() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.limits.default_results_per_page = 200;
        config.limits.max_results_per_page = 50;
        config.limits.max_line_bytes = 1024;
        config.limits.max_returned_content_bytes = 512;
        config.limits.max_response_bytes = 512;

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(
            &error,
            "limits.default_results_per_page"
        ));
        assert!(has_issue(&error, "limits.max_line_bytes"));
        assert!(has_issue(
            &error,
            "limits.max_returned_content_bytes"
        ));
    }

    #[test]
    fn rejects_invalid_source_id_and_relative_root() {
        let mut config = AppConfig::from_json_str(EXAMPLE).expect("example should parse");
        config.sources[0].source_id = "payment source".to_owned();
        config.sources[0].root = PathBuf::from("var/log/payment");

        let error = config.validate().expect_err("configuration should fail");
        assert!(has_issue(&error, "sources[0].source_id"));
        assert!(has_issue(&error, "sources[0].root"));
    }

    fn has_issue(error: &ConfigValidationError, field: &str) -> bool {
        error.issues().iter().any(|issue| issue.field == field)
    }
}
