use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::Arc,
};

use schemars::JsonSchema;
use serde::Serialize;
use thiserror::Error;

use crate::{
    AppConfig, ConfigValidationError, LogSourceConfig, SafeFile, SafeOpenError, SafeRoot,
    SourceDiscoveryError, TimestampRule,
    source_discovery::{DirectoryDiscoveryRule, MAX_DISCOVERED_FILES, discover_regular_files},
};

#[derive(Debug, Clone, Serialize, JsonSchema, PartialEq, Eq)]
pub struct LogSourceDescriptor {
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
    descriptor: LogSourceDescriptor,
    root: Arc<SafeRoot>,
    files: Vec<ConfiguredFile>,
    files_by_id: HashMap<String, usize>,
    timestamp_rule: Option<TimestampRule>,
}

impl ConfiguredLogSource {
    #[must_use]
    pub const fn descriptor(&self) -> &LogSourceDescriptor {
        &self.descriptor
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
    pub fn timestamp_rule(&self) -> Option<&TimestampRule> {
        self.timestamp_rule.as_ref()
    }

    #[must_use]
    pub fn file(&self, file_id: &str) -> Option<&ConfiguredFile> {
        self.files_by_id
            .get(file_id)
            .and_then(|index| self.files.get(*index))
    }

    #[must_use]
    pub fn contains_relative_path(&self, path: &Path) -> bool {
        self.files
            .binary_search_by(|candidate| candidate.relative_path.as_path().cmp(path))
            .is_ok()
    }

    pub fn open_file(&self, file_id: &str) -> Result<SafeFile, ConfiguredFileError> {
        let file = self
            .file(file_id)
            .ok_or(ConfiguredFileError::UnknownFile)?;
        Ok(self.root.open_regular_file(&file.relative_path)?)
    }
}

#[derive(Debug)]
pub struct SourceRegistry {
    sources: Vec<Arc<ConfiguredLogSource>>,
    by_id: HashMap<String, usize>,
}

impl SourceRegistry {
    pub fn from_config(config: &AppConfig) -> Result<Self, SourceRegistryError> {
        config.validate()?;

        let enabled_sources: Vec<&LogSourceConfig> =
            config.sources.iter().filter(|source| source.enabled).collect();
        if enabled_sources.is_empty() {
            return Err(SourceRegistryError::NoEnabledSources);
        }

        let mut sources = Vec::with_capacity(enabled_sources.len());
        let mut by_id = HashMap::with_capacity(enabled_sources.len());

        for (source_index, source) in enabled_sources.into_iter().enumerate() {
            let configured = Arc::new(load_source(source_index, source)?);
            by_id.insert(source.source_id.clone(), sources.len());
            sources.push(configured);
        }

        Ok(Self { sources, by_id })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }

    #[must_use]
    pub fn list(&self) -> Vec<LogSourceDescriptor> {
        self.sources
            .iter()
            .map(|source| source.descriptor().clone())
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
        source_ids
            .iter()
            .map(|source_id| {
                self.get(source_id)
                    .ok_or_else(|| SourceRegistryError::UnknownSource(source_id.clone()))
            })
            .collect()
    }
}

fn load_source(
    source_index: usize,
    source: &LogSourceConfig,
) -> Result<ConfiguredLogSource, SourceRegistryError> {
    let root = Arc::new(SafeRoot::open(&source.root).map_err(|error| {
        SourceRegistryError::SourceRoot {
            source_id: source.source_id.clone(),
            source: error,
        }
    })?);

    let mut paths = BTreeSet::new();
    for relative_path in &source.files {
        root.open_regular_file(relative_path).map_err(|error| {
            SourceRegistryError::ExplicitFile {
                source_id: source.source_id.clone(),
                source: error,
            }
        })?;
        paths.insert(relative_path.clone());
    }

    if !source.directories.is_empty() {
        let rules = source
            .directories
            .iter()
            .map(DirectoryDiscoveryRule::from_config)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| SourceRegistryError::Discovery {
                source_id: source.source_id.clone(),
                source: error,
            })?;
        let remaining = MAX_DISCOVERED_FILES.saturating_sub(paths.len());
        let discovered = discover_regular_files(&root, &rules, remaining).map_err(|error| {
            SourceRegistryError::Discovery {
                source_id: source.source_id.clone(),
                source: error,
            }
        })?;
        paths.extend(discovered);
    }

    if paths.len() > MAX_DISCOVERED_FILES {
        return Err(SourceRegistryError::TooManyFiles {
            source_id: source.source_id.clone(),
        });
    }

    let mut files = Vec::with_capacity(paths.len());
    let mut files_by_id = HashMap::with_capacity(paths.len());
    for (file_index, relative_path) in paths.into_iter().enumerate() {
        let file_id = format!("file-{source_index}-{file_index}");
        let display_name = relative_path.to_string_lossy().into_owned();
        files_by_id.insert(file_id.clone(), file_index);
        files.push(ConfiguredFile {
            file_id,
            relative_path,
            display_name,
        });
    }

    Ok(ConfiguredLogSource {
        descriptor: LogSourceDescriptor {
            source_id: source.source_id.clone(),
            name: source.name.clone(),
            description: source.description.clone(),
            service: source.service.clone(),
            environment: source.environment.clone(),
            tags: source.tags.clone(),
        },
        root,
        files,
        files_by_id,
        timestamp_rule: source.timestamp_rule.clone(),
    })
}

#[derive(Debug, Error)]
pub enum SourceRegistryError {
    #[error(transparent)]
    InvalidConfig(#[from] ConfigValidationError),

    #[error("configuration must contain at least one enabled log source")]
    NoEnabledSources,

    #[error("configured log source root cannot be opened safely: {source_id}")]
    SourceRoot {
        source_id: String,
        #[source]
        source: SafeOpenError,
    },

    #[error("configured explicit log file cannot be opened safely: {source_id}")]
    ExplicitFile {
        source_id: String,
        #[source]
        source: SafeOpenError,
    },

    #[error("configured log directory cannot be discovered safely: {source_id}")]
    Discovery {
        source_id: String,
        #[source]
        source: SourceDiscoveryError,
    },

    #[error("configured log source resolves to too many files: {source_id}")]
    TooManyFiles { source_id: String },

    #[error("unknown log source: {0}")]
    UnknownSource(String),
}

#[derive(Debug, Error)]
pub enum ConfiguredFileError {
    #[error("unknown configured file identifier")]
    UnknownFile,

    #[error("configured log file cannot be opened safely")]
    SafeOpen(#[from] SafeOpenError),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use crate::{DirectoryRule, Encoding, LimitsConfig, CONFIG_VERSION};

    use super::*;

    fn source(root: PathBuf) -> LogSourceConfig {
        LogSourceConfig {
            source_id: "payment-test".to_owned(),
            name: "Payment test".to_owned(),
            description: "payment logs".to_owned(),
            service: "payment-service".to_owned(),
            environment: "test".to_owned(),
            tags: vec!["payment".to_owned()],
            enabled: true,
            encoding: Encoding::Utf8,
            root,
            files: vec![PathBuf::from("application.log")],
            directories: Vec::new(),
            timestamp_rule: None,
        }
    }

    fn app_config(sources: Vec<LogSourceConfig>) -> AppConfig {
        AppConfig {
            version: CONFIG_VERSION,
            sources,
            limits: LimitsConfig::default(),
        }
    }

    #[test]
    fn loads_explicit_and_discovered_files_in_stable_order() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "current\n")
            .expect("current log should be written");
        fs::write(directory.path().join("audit.log"), "audit\n")
            .expect("audit log should be written");
        fs::write(directory.path().join("notes.txt"), "notes\n")
            .expect("notes should be written");
        let mut configured = source(directory.path().to_path_buf());
        configured.directories = vec![DirectoryRule {
            path: PathBuf::from("."),
            recursive: false,
            include_suffixes: vec![".log".to_owned()],
        }];

        let registry = SourceRegistry::from_config(&app_config(vec![configured]))
            .expect("registry should load");
        let loaded = registry.get("payment-test").expect("source should exist");

        assert_eq!(registry.list()[0].source_id, "payment-test");
        assert_eq!(
            loaded
                .files()
                .iter()
                .map(ConfiguredFile::display_name)
                .collect::<Vec<_>>(),
            vec!["application.log", "audit.log"]
        );
        assert_eq!(loaded.files()[0].file_id(), "file-0-0");
        assert_eq!(loaded.files()[1].file_id(), "file-0-1");
        assert!(loaded.contains_relative_path(Path::new("application.log")));
    }

    #[test]
    fn disabled_source_is_not_opened_or_exposed() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "current\n")
            .expect("current log should be written");
        let enabled = source(directory.path().to_path_buf());
        let mut disabled = source(PathBuf::from("/path/that/does/not/exist"));
        disabled.source_id = "disabled-test".to_owned();
        disabled.enabled = false;

        let registry = SourceRegistry::from_config(&app_config(vec![enabled, disabled]))
            .expect("disabled source should not block startup");

        assert_eq!(registry.len(), 1);
        assert!(registry.get("disabled-test").is_none());
    }

    #[test]
    fn rejects_configuration_without_enabled_sources() {
        let mut disabled = source(PathBuf::from("/path/that/does/not/exist"));
        disabled.enabled = false;

        assert!(matches!(
            SourceRegistry::from_config(&app_config(vec![disabled])),
            Err(SourceRegistryError::NoEnabledSources)
        ));
    }

    #[test]
    fn rejects_missing_or_symlinked_explicit_file() {
        let directory = tempdir().expect("temporary directory should be created");
        let mut missing = source(directory.path().to_path_buf());
        assert!(matches!(
            SourceRegistry::from_config(&app_config(vec![missing.clone()])),
            Err(SourceRegistryError::ExplicitFile { .. })
        ));

        let outside = tempdir().expect("outside directory should be created");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("outside fixture should be written");
        symlink(
            outside.path().join("secret.log"),
            directory.path().join("application.log"),
        )
        .expect("symlink should be created");
        missing.root = directory.path().to_path_buf();
        assert!(matches!(
            SourceRegistry::from_config(&app_config(vec![missing])),
            Err(SourceRegistryError::ExplicitFile { .. })
        ));
    }

    #[test]
    fn rejects_missing_discovery_directory() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "current\n")
            .expect("current log should be written");
        let mut configured = source(directory.path().to_path_buf());
        configured.directories = vec![DirectoryRule {
            path: PathBuf::from("archive"),
            recursive: true,
            include_suffixes: vec![".log".to_owned()],
        }];

        assert!(matches!(
            SourceRegistry::from_config(&app_config(vec![configured])),
            Err(SourceRegistryError::Discovery { .. })
        ));
    }

    #[test]
    fn revalidates_configured_file_on_every_open() {
        let directory = tempdir().expect("temporary directory should be created");
        let outside = tempdir().expect("outside directory should be created");
        let path = directory.path().join("application.log");
        fs::write(&path, "current\n").expect("current log should be written");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("outside fixture should be written");
        let registry = SourceRegistry::from_config(&app_config(vec![source(
            directory.path().to_path_buf(),
        )]))
        .expect("registry should load");
        let loaded = registry.get("payment-test").expect("source should exist");
        let file_id = loaded.files()[0].file_id().to_owned();

        fs::remove_file(&path).expect("configured file should be removed");
        symlink(outside.path().join("secret.log"), &path)
            .expect("replacement symlink should be created");

        assert!(matches!(
            loaded.open_file(&file_id),
            Err(ConfiguredFileError::SafeOpen(_))
        ));
    }

    #[test]
    fn selected_rejects_unknown_source() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "current\n")
            .expect("current log should be written");
        let registry = SourceRegistry::from_config(&app_config(vec![source(
            directory.path().to_path_buf(),
        )]))
        .expect("registry should load");

        assert!(matches!(
            registry.selected(&["unknown".to_owned()]),
            Err(SourceRegistryError::UnknownSource(source_id)) if source_id == "unknown"
        ));
    }
}
