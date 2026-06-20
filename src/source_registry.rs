use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use crate::{
    AppConfig, ConfigValidationError, SafeOpenError, SafeRoot, SourceDiscoveryError, TimestampRule,
    discover_regular_files,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogSourceInfo {
    pub source_id: String,
    pub name: String,
    pub description: String,
    pub service: String,
    pub environment: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredFile {
    file_id: String,
    relative_path: PathBuf,
    display_name: String,
}

impl ConfiguredFile {
    #[must_use]
    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    #[must_use]
    pub fn display_name(&self) -> &str {
        &self.display_name
    }
}

#[derive(Debug)]
pub struct ConfiguredLogSource {
    public: LogSourceInfo,
    root: Arc<SafeRoot>,
    files: Vec<ConfiguredFile>,
    files_by_id: HashMap<String, usize>,
    timestamp_rule: Option<TimestampRule>,
}

impl ConfiguredLogSource {
    #[must_use]
    pub fn public(&self) -> &LogSourceInfo {
        &self.public
    }

    #[must_use]
    pub fn root(&self) -> Arc<SafeRoot> {
        Arc::clone(&self.root)
    }

    #[must_use]
    pub fn files(&self) -> &[ConfiguredFile] {
        &self.files
    }

    #[must_use]
    pub fn file_by_id(&self, file_id: &str) -> Option<&ConfiguredFile> {
        self.files_by_id
            .get(file_id)
            .and_then(|index| self.files.get(*index))
    }

    #[must_use]
    pub fn file_by_relative_path(&self, relative_path: &Path) -> Option<&ConfiguredFile> {
        self.files
            .iter()
            .find(|file| file.relative_path == relative_path)
    }

    #[must_use]
    pub fn timestamp_rule(&self) -> Option<&TimestampRule> {
        self.timestamp_rule.as_ref()
    }
}

#[derive(Debug)]
pub struct SourceRegistry {
    sources: Vec<Arc<ConfiguredLogSource>>,
    by_id: HashMap<String, usize>,
}

impl SourceRegistry {
    pub fn build(config: &AppConfig) -> Result<Self, SourceRegistryError> {
        config.validate()?;

        let enabled_sources = config.sources.iter().filter(|source| source.enabled);
        let mut sources = Vec::new();
        let mut by_id = HashMap::new();

        for source in enabled_sources {
            let root = Arc::new(SafeRoot::open(&source.root).map_err(|error| {
                SourceRegistryError::SourceOpen {
                    source_id: source.source_id.clone(),
                    source: error,
                }
            })?);

            let file_limit = config.limits.max_scan_files_per_query;
            if source.files.len() > file_limit {
                return Err(SourceRegistryError::TooManyFiles {
                    source_id: source.source_id.clone(),
                    limit: file_limit,
                });
            }

            let mut paths = Vec::with_capacity(source.files.len());
            let mut unique_paths = HashSet::with_capacity(source.files.len());
            for relative_path in &source.files {
                root.open_regular_file(relative_path).map_err(|error| {
                    SourceRegistryError::SourceOpen {
                        source_id: source.source_id.clone(),
                        source: error,
                    }
                })?;
                if unique_paths.insert(relative_path.clone()) {
                    paths.push(relative_path.clone());
                }
            }

            if !source.directories.is_empty() {
                let remaining = file_limit.saturating_sub(paths.len());
                let discovered = discover_regular_files(&root, &source.directories, remaining)
                    .map_err(|error| SourceRegistryError::Discovery {
                        source_id: source.source_id.clone(),
                        source: error,
                    })?;
                for relative_path in discovered {
                    if unique_paths.insert(relative_path.clone()) {
                        paths.push(relative_path);
                    }
                }
            }

            if paths.is_empty() {
                return Err(SourceRegistryError::NoResolvedFiles(
                    source.source_id.clone(),
                ));
            }
            if paths.len() > file_limit {
                return Err(SourceRegistryError::TooManyFiles {
                    source_id: source.source_id.clone(),
                    limit: file_limit,
                });
            }

            paths.sort();
            let files = paths
                .into_iter()
                .enumerate()
                .map(|(index, relative_path)| ConfiguredFile {
                    file_id: format!("file-{}-{index}", source.source_id),
                    display_name: relative_path.to_string_lossy().into_owned(),
                    relative_path,
                })
                .collect::<Vec<_>>();
            let files_by_id = files
                .iter()
                .enumerate()
                .map(|(index, file)| (file.file_id.clone(), index))
                .collect();

            let public = LogSourceInfo {
                source_id: source.source_id.clone(),
                name: source.name.clone(),
                description: source.description.clone(),
                service: source.service.clone(),
                environment: source.environment.clone(),
                tags: source.tags.clone(),
            };
            let index = sources.len();
            by_id.insert(source.source_id.clone(), index);
            sources.push(Arc::new(ConfiguredLogSource {
                public,
                root,
                files,
                files_by_id,
                timestamp_rule: source.timestamp_rule.clone(),
            }));
        }

        if sources.is_empty() {
            return Err(SourceRegistryError::NoEnabledSources);
        }

        Ok(Self { sources, by_id })
    }

    #[must_use]
    pub fn list(&self) -> Vec<LogSourceInfo> {
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
    ) -> Result<Vec<Arc<ConfiguredLogSource>>, SourceRegistryError> {
        let mut seen = HashSet::with_capacity(source_ids.len());
        let mut selected = Vec::with_capacity(source_ids.len());

        for source_id in source_ids {
            if !seen.insert(source_id.as_str()) {
                return Err(SourceRegistryError::DuplicateRequestedSource(
                    source_id.clone(),
                ));
            }
            selected.push(
                self.get(source_id)
                    .ok_or_else(|| SourceRegistryError::UnknownSource(source_id.clone()))?,
            );
        }
        Ok(selected)
    }
}

#[derive(Debug, Error)]
pub enum SourceRegistryError {
    #[error(transparent)]
    InvalidConfig(#[from] ConfigValidationError),

    #[error("configuration does not contain an enabled log source")]
    NoEnabledSources,

    #[error("configured log source cannot be opened safely: {source_id}")]
    SourceOpen {
        source_id: String,
        #[source]
        source: SafeOpenError,
    },

    #[error("configured log source cannot be discovered safely: {source_id}")]
    Discovery {
        source_id: String,
        #[source]
        source: SourceDiscoveryError,
    },

    #[error("configured log source resolves more than {limit} files: {source_id}")]
    TooManyFiles { source_id: String, limit: usize },

    #[error("configured log source does not resolve an ordinary file: {0}")]
    NoResolvedFiles(String),

    #[error("unknown log source: {0}")]
    UnknownSource(String),

    #[error("requested log source is duplicated: {0}")]
    DuplicateRequestedSource(String),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use crate::{DirectoryRule, Encoding, LimitsConfig, LogSourceConfig};

    use super::*;

    fn source(source_id: &str, root: &Path) -> LogSourceConfig {
        LogSourceConfig {
            source_id: source_id.to_owned(),
            name: format!("{source_id} name"),
            description: String::new(),
            service: source_id.to_owned(),
            environment: "test".to_owned(),
            tags: vec!["test".to_owned()],
            enabled: true,
            encoding: Encoding::Utf8,
            root: root.to_path_buf(),
            files: Vec::new(),
            directories: Vec::new(),
            timestamp_rule: None,
        }
    }

    fn config(sources: Vec<LogSourceConfig>) -> AppConfig {
        AppConfig {
            version: 1,
            sources,
            limits: LimitsConfig::default(),
        }
    }

    #[test]
    fn builds_registry_from_explicit_and_discovered_files() {
        let payment = tempdir().expect("payment root should be created");
        fs::create_dir(payment.path().join("archive")).expect("archive should be created");
        fs::write(payment.path().join("application.log"), "current\n")
            .expect("fixture should be written");
        fs::write(payment.path().join("archive/application.log.1"), "old\n")
            .expect("fixture should be written");
        fs::write(payment.path().join("archive/notes.txt"), "ignored\n")
            .expect("fixture should be written");

        let mut payment_source = source("payment-test", payment.path());
        payment_source.files = vec![PathBuf::from("application.log")];
        payment_source.directories = vec![DirectoryRule {
            path: PathBuf::from("archive"),
            recursive: false,
            include_suffixes: vec![".log.1".to_owned()],
        }];

        let registry =
            SourceRegistry::build(&config(vec![payment_source])).expect("registry should build");
        let configured = registry
            .get("payment-test")
            .expect("source should be available");

        assert_eq!(registry.list().len(), 1);
        assert_eq!(configured.files().len(), 2);
        assert_eq!(configured.files()[0].display_name(), "application.log");
        assert_eq!(
            configured.files()[1].display_name(),
            "archive/application.log.1"
        );
        assert_eq!(configured.files()[0].file_id(), "file-payment-test-0");
    }

    #[test]
    fn disabled_sources_are_not_opened_or_listed() {
        let enabled_root = tempdir().expect("enabled root should be created");
        fs::write(enabled_root.path().join("application.log"), "current\n")
            .expect("fixture should be written");
        let mut enabled = source("enabled", enabled_root.path());
        enabled.files = vec![PathBuf::from("application.log")];

        let mut disabled = source("disabled", Path::new("/path/that/does/not/exist"));
        disabled.enabled = false;
        disabled.files = vec![PathBuf::from("missing.log")];

        let registry = SourceRegistry::build(&config(vec![enabled, disabled]))
            .expect("disabled source should not block registry creation");
        assert_eq!(registry.list().len(), 1);
        assert!(registry.get("enabled").is_some());
        assert!(registry.get("disabled").is_none());
    }

    #[test]
    fn rejects_explicit_file_symlink() {
        let root = tempdir().expect("source root should be created");
        let outside = tempdir().expect("outside root should be created");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("fixture should be written");
        symlink(
            outside.path().join("secret.log"),
            root.path().join("application.log"),
        )
        .expect("symlink should be created");

        let mut configured = source("payment-test", root.path());
        configured.files = vec![PathBuf::from("application.log")];
        assert!(matches!(
            SourceRegistry::build(&config(vec![configured])),
            Err(SourceRegistryError::SourceOpen { .. })
        ));
    }

    #[test]
    fn rejects_source_without_resolved_files() {
        let root = tempdir().expect("source root should be created");
        let mut configured = source("payment-test", root.path());
        configured.directories = vec![DirectoryRule {
            path: PathBuf::from("."),
            recursive: false,
            include_suffixes: vec![".log".to_owned()],
        }];

        assert!(matches!(
            SourceRegistry::build(&config(vec![configured])),
            Err(SourceRegistryError::NoResolvedFiles(source_id))
                if source_id == "payment-test"
        ));
    }

    #[test]
    fn selection_preserves_request_order_and_rejects_invalid_selection() {
        let first_root = tempdir().expect("first root should be created");
        let second_root = tempdir().expect("second root should be created");
        fs::write(first_root.path().join("one.log"), "one\n").expect("fixture should be written");
        fs::write(second_root.path().join("two.log"), "two\n").expect("fixture should be written");
        let mut first = source("first", first_root.path());
        first.files = vec![PathBuf::from("one.log")];
        let mut second = source("second", second_root.path());
        second.files = vec![PathBuf::from("two.log")];
        let registry =
            SourceRegistry::build(&config(vec![first, second])).expect("registry should build");

        let selected = registry
            .selected(&["second".to_owned(), "first".to_owned()])
            .expect("selection should work");
        assert_eq!(selected[0].public().source_id, "second");
        assert_eq!(selected[1].public().source_id, "first");

        assert!(matches!(
            registry.selected(&["missing".to_owned()]),
            Err(SourceRegistryError::UnknownSource(source_id)) if source_id == "missing"
        ));
        assert!(matches!(
            registry.selected(&["first".to_owned(), "first".to_owned()]),
            Err(SourceRegistryError::DuplicateRequestedSource(source_id)) if source_id == "first"
        ));
    }

    #[test]
    fn rejects_configuration_with_no_enabled_sources() {
        let root = tempdir().expect("source root should be created");
        let mut disabled = source("disabled", root.path());
        disabled.enabled = false;
        disabled.files = vec![PathBuf::from("application.log")];

        assert!(matches!(
            SourceRegistry::build(&config(vec![disabled])),
            Err(SourceRegistryError::NoEnabledSources)
        ));
    }
}
