use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Component, Path, PathBuf},
    sync::Arc,
};

use serde::Deserialize;
use thiserror::Error;

use crate::{
    DirectoryDiscoveryRule, LogSource, MAX_CURSOR_CANDIDATE_FILES, MAX_DIRECTORY_RULES_PER_SOURCE,
    MAX_SOURCE_ID_CHARS, SafeOpenError, SafeRoot, SourceDiscoveryError, TimeFilterError,
    TimestampRule, discover_regular_files,
};

pub const MAX_CONFIGURED_SOURCES: usize = 100;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceConfig {
    pub sources: Vec<LogSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
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
    /// Trusted administrator configuration. This path is never exposed through MCP.
    pub root: PathBuf,
    /// Explicit normalized paths relative to `root`.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// Bounded directory discovery rules relative to `root`.
    #[serde(default)]
    pub directories: Vec<DirectorySourceConfig>,
    pub timestamp_rule: Option<TimestampRuleConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DirectorySourceConfig {
    /// `.` means the source root; other values must be normalized relative paths.
    pub path: PathBuf,
    #[serde(default)]
    pub recursive: bool,
    /// Case-sensitive filename suffixes, for example `.log` and `.log.1`.
    pub include_suffixes: Vec<String>,
}

impl DirectorySourceConfig {
    fn build(&self) -> Result<DirectoryDiscoveryRule, RuntimeConfigError> {
        Ok(DirectoryDiscoveryRule::new(
            self.path.clone(),
            self.recursive,
            self.include_suffixes.clone(),
        )?)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TimestampRuleConfig {
    Rfc3339 {
        prefix_bytes: usize,
    },
    Custom {
        prefix_bytes: usize,
        format: String,
        default_offset_seconds: Option<i32>,
    },
}

impl TimestampRuleConfig {
    fn build(self) -> Result<TimestampRule, RuntimeConfigError> {
        let rule = match self {
            Self::Rfc3339 { prefix_bytes } => TimestampRule::Rfc3339 { prefix_bytes },
            Self::Custom {
                prefix_bytes,
                format,
                default_offset_seconds,
            } => TimestampRule::Custom {
                prefix_bytes,
                format,
                default_offset_seconds,
            },
        };
        rule.validate()?;
        Ok(rule)
    }
}

#[derive(Debug)]
pub struct ConfiguredLogSource {
    public: LogSource,
    root: Arc<SafeRoot>,
    files: Vec<PathBuf>,
    timestamp_rule: Option<TimestampRule>,
}

impl ConfiguredLogSource {
    #[must_use]
    pub fn public(&self) -> &LogSource {
        &self.public
    }

    #[must_use]
    pub fn root(&self) -> Arc<SafeRoot> {
        Arc::clone(&self.root)
    }

    #[must_use]
    pub fn files(&self) -> &[PathBuf] {
        &self.files
    }

    #[must_use]
    pub fn timestamp_rule(&self) -> Option<&TimestampRule> {
        self.timestamp_rule.as_ref()
    }

    #[must_use]
    pub fn file_index(&self, relative_path: &Path) -> Option<usize> {
        self.files
            .iter()
            .position(|candidate| candidate == relative_path)
    }
}

#[derive(Debug)]
pub struct SourceRegistry {
    sources: Vec<Arc<ConfiguredLogSource>>,
    by_id: HashMap<String, usize>,
}

impl SourceRegistry {
    pub fn from_config_path(path: impl AsRef<Path>) -> Result<Self, RuntimeConfigError> {
        let path = path.as_ref();
        let content = fs::read_to_string(path).map_err(RuntimeConfigError::ReadConfig)?;
        let config: ServiceConfig =
            serde_json::from_str(&content).map_err(RuntimeConfigError::ParseConfig)?;
        let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
        Self::from_config(config, base_dir)
    }

    pub fn from_config(
        config: ServiceConfig,
        base_dir: impl AsRef<Path>,
    ) -> Result<Self, RuntimeConfigError> {
        if config.sources.is_empty() || config.sources.len() > MAX_CONFIGURED_SOURCES {
            return Err(RuntimeConfigError::InvalidConfig(
                "configured source count is outside the service limit",
            ));
        }

        let base_dir = base_dir.as_ref();
        let mut source_ids = HashSet::with_capacity(config.sources.len());
        let mut sources = Vec::with_capacity(config.sources.len());
        let mut by_id = HashMap::with_capacity(config.sources.len());

        for source in config.sources {
            validate_source_id(&source.source_id)?;
            if !source_ids.insert(source.source_id.clone()) {
                return Err(RuntimeConfigError::DuplicateSourceId(source.source_id));
            }
            if source.name.is_empty() || source.service.is_empty() || source.environment.is_empty()
            {
                return Err(RuntimeConfigError::InvalidConfig(
                    "source name, service and environment must not be empty",
                ));
            }
            if source.files.is_empty() && source.directories.is_empty() {
                return Err(RuntimeConfigError::InvalidConfig(
                    "source must configure at least one file or directory",
                ));
            }
            if source.files.len() > MAX_CURSOR_CANDIDATE_FILES
                || source.directories.len() > MAX_DIRECTORY_RULES_PER_SOURCE
            {
                return Err(RuntimeConfigError::InvalidConfig(
                    "source file or directory rule count is outside the service limit",
                ));
            }

            let root_path = if source.root.is_absolute() {
                source.root.clone()
            } else {
                base_dir.join(&source.root)
            };
            let root = Arc::new(SafeRoot::open(root_path)?);

            let mut files = Vec::with_capacity(source.files.len());
            let mut unique_files = HashSet::with_capacity(source.files.len());
            for relative_path in &source.files {
                validate_relative_path(relative_path)?;
                if !unique_files.insert(relative_path.clone()) {
                    return Err(RuntimeConfigError::InvalidConfig(
                        "source contains a duplicate relative file path",
                    ));
                }
                root.open_regular_file(relative_path)?;
                files.push(relative_path.clone());
            }

            if !source.directories.is_empty() {
                let rules = source
                    .directories
                    .iter()
                    .map(DirectorySourceConfig::build)
                    .collect::<Result<Vec<_>, RuntimeConfigError>>()?;
                let remaining = MAX_CURSOR_CANDIDATE_FILES.saturating_sub(files.len());
                let discovered = discover_regular_files(&root, &rules, remaining)?;
                for relative_path in discovered {
                    if unique_files.insert(relative_path.clone()) {
                        files.push(relative_path);
                    }
                }
            }

            if files.len() > MAX_CURSOR_CANDIDATE_FILES {
                return Err(RuntimeConfigError::InvalidConfig(
                    "source resolved file count is outside the service limit",
                ));
            }
            files.sort();

            let timestamp_rule = source
                .timestamp_rule
                .map(TimestampRuleConfig::build)
                .transpose()?;
            let public = LogSource {
                source_id: source.source_id.clone(),
                name: source.name,
                description: source.description,
                service: source.service,
                environment: source.environment,
                tags: source.tags,
            };
            let index = sources.len();
            by_id.insert(source.source_id, index);
            sources.push(Arc::new(ConfiguredLogSource {
                public,
                root,
                files,
                timestamp_rule,
            }));
        }

        Ok(Self { sources, by_id })
    }

    #[must_use]
    pub fn list(&self) -> Vec<LogSource> {
        self.sources
            .iter()
            .map(|source| source.public().clone())
            .collect()
    }

    #[must_use]
    pub fn get(&self, source_id: &str) -> Option<Arc<ConfiguredLogSource>> {
        self.by_id
            .get(source_id)
            .and_then(|index| self.sources.get(*index))
            .map(Arc::clone)
    }

    pub fn selected(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<Arc<ConfiguredLogSource>>, RuntimeConfigError> {
        source_ids
            .iter()
            .map(|source_id| {
                self.get(source_id)
                    .ok_or_else(|| RuntimeConfigError::UnknownSource(source_id.clone()))
            })
            .collect()
    }
}

fn validate_source_id(source_id: &str) -> Result<(), RuntimeConfigError> {
    let chars = source_id.chars().count();
    if chars == 0 || chars > MAX_SOURCE_ID_CHARS {
        return Err(RuntimeConfigError::InvalidConfig(
            "source_id length is outside the service limit",
        ));
    }
    Ok(())
}

fn validate_relative_path(path: &Path) -> Result<(), RuntimeConfigError> {
    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(RuntimeConfigError::InvalidConfig(
                    "configured file paths must be normalized and relative to their source root",
                ));
            }
        }
    }
    if !has_component {
        return Err(RuntimeConfigError::InvalidConfig(
            "configured file path must contain a file name",
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum RuntimeConfigError {
    #[error("cannot read log-query MCP configuration")]
    ReadConfig(#[source] std::io::Error),

    #[error("log-query MCP configuration is not valid JSON")]
    ParseConfig(#[source] serde_json::Error),

    #[error("invalid log-query MCP configuration: {0}")]
    InvalidConfig(&'static str),

    #[error("duplicate log source identifier: {0}")]
    DuplicateSourceId(String),

    #[error("unknown log source: {0}")]
    UnknownSource(String),

    #[error("configured log file cannot be opened safely")]
    SafeOpen(#[from] SafeOpenError),

    #[error("configured log directory cannot be discovered safely")]
    Discovery(#[from] SourceDiscoveryError),

    #[error("configured timestamp rule is invalid")]
    Timestamp(#[from] TimeFilterError),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::*;

    fn source(root: PathBuf) -> LogSourceConfig {
        LogSourceConfig {
            source_id: "payment-test".to_owned(),
            name: "payment test".to_owned(),
            description: String::new(),
            service: "payment".to_owned(),
            environment: "test".to_owned(),
            tags: vec!["java".to_owned()],
            root,
            files: vec![PathBuf::from("application.log")],
            directories: Vec::new(),
            timestamp_rule: Some(TimestampRuleConfig::Rfc3339 { prefix_bytes: 64 }),
        }
    }

    #[test]
    fn loads_explicit_regular_files_without_exposing_roots() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "hello\n")
            .expect("fixture should be written");
        let registry = SourceRegistry::from_config(
            ServiceConfig {
                sources: vec![source(directory.path().to_path_buf())],
            },
            ".",
        )
        .expect("registry should load");

        assert_eq!(registry.list().len(), 1);
        assert_eq!(registry.list()[0].source_id, "payment-test");
        assert!(registry.get("payment-test").is_some());
    }

    #[test]
    fn loads_directory_rules_and_skips_symlinks() {
        let directory = tempdir().expect("temporary directory should be created");
        let outside = tempdir().expect("outside directory should be created");
        fs::create_dir_all(directory.path().join("logs/archive"))
            .expect("log directories should be created");
        fs::write(directory.path().join("logs/application.log"), "current\n")
            .expect("current log should be written");
        fs::write(
            directory.path().join("logs/archive/application.log.1"),
            "rotated\n",
        )
        .expect("rotated log should be written");
        fs::write(directory.path().join("logs/notes.txt"), "ignore\n")
            .expect("non-log file should be written");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("outside file should be written");
        symlink(
            outside.path().join("secret.log"),
            directory.path().join("logs/linked.log"),
        )
        .expect("symlink should be created");

        let mut config = source(directory.path().to_path_buf());
        config.files.clear();
        config.directories = vec![DirectorySourceConfig {
            path: PathBuf::from("logs"),
            recursive: true,
            include_suffixes: vec![".log".to_owned(), ".log.1".to_owned()],
        }];
        let registry = SourceRegistry::from_config(
            ServiceConfig {
                sources: vec![config],
            },
            ".",
        )
        .expect("directory source should load");
        let configured = registry.get("payment-test").expect("source should exist");

        assert_eq!(
            configured.files(),
            &[
                PathBuf::from("logs/application.log"),
                PathBuf::from("logs/archive/application.log.1")
            ]
        );
    }

    #[test]
    fn rejects_parent_paths_and_duplicate_source_ids() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "hello\n")
            .expect("fixture should be written");
        let mut invalid = source(directory.path().to_path_buf());
        invalid.files = vec![PathBuf::from("../outside.log")];
        assert!(matches!(
            SourceRegistry::from_config(
                ServiceConfig {
                    sources: vec![invalid]
                },
                "."
            ),
            Err(RuntimeConfigError::InvalidConfig(_))
        ));

        let valid = source(directory.path().to_path_buf());
        assert!(matches!(
            SourceRegistry::from_config(
                ServiceConfig {
                    sources: vec![valid.clone(), valid]
                },
                "."
            ),
            Err(RuntimeConfigError::DuplicateSourceId(_))
        ));
    }
}
