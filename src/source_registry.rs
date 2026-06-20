use std::{
    collections::{BTreeMap, HashMap},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use crate::{
    AppConfig, ConfigValidationError, DirectoryDiscoveryRule, DirectoryRule, FileIdentity,
    LimitsConfig, SafeFile, SafeOpenError, SafeRoot, SourceDiscoveryError, TimestampRule,
    discover_regular_files,
};

pub const MAX_REGISTERED_FILES_PER_SOURCE: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceDescriptor {
    pub source_id: String,
    pub name: String,
    pub description: String,
    pub service: String,
    pub environment: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFileSnapshot {
    source_id: String,
    file_id: String,
    relative_path: PathBuf,
    identity: FileIdentity,
    size_at_snapshot: u64,
}

impl SourceFileSnapshot {
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    #[must_use]
    pub fn file_id(&self) -> &str {
        &self.file_id
    }

    #[must_use]
    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    #[must_use]
    pub const fn size_at_snapshot(&self) -> u64 {
        self.size_at_snapshot
    }

    #[must_use]
    pub fn display_name(&self) -> String {
        self.relative_path.to_string_lossy().into_owned()
    }
}

#[derive(Debug)]
pub struct ConfiguredSource {
    descriptor: SourceDescriptor,
    root: Arc<SafeRoot>,
    explicit_files: Vec<PathBuf>,
    directory_configs: Vec<DirectoryRule>,
    discovery_rules: Vec<DirectoryDiscoveryRule>,
    timestamp_rule: Option<TimestampRule>,
}

impl ConfiguredSource {
    #[must_use]
    pub fn descriptor(&self) -> &SourceDescriptor {
        &self.descriptor
    }

    #[must_use]
    pub fn timestamp_rule(&self) -> Option<&TimestampRule> {
        self.timestamp_rule.as_ref()
    }

    pub fn snapshot_files(
        &self,
        max_files: usize,
    ) -> Result<Vec<SourceFileSnapshot>, SourceRegistryError> {
        if max_files == 0 || max_files > MAX_REGISTERED_FILES_PER_SOURCE {
            return Err(SourceRegistryError::TooManyFiles {
                source_id: self.descriptor.source_id.clone(),
                limit: max_files,
            });
        }

        let mut candidates = BTreeMap::<PathBuf, (FileIdentity, u64)>::new();
        for (index, relative_path) in self.explicit_files.iter().enumerate() {
            let file = self
                .root
                .open_regular_file(relative_path)
                .map_err(|source| SourceRegistryError::ExplicitFileUnavailable {
                    source_id: self.descriptor.source_id.clone(),
                    file_index: index,
                    source,
                })?;
            candidates.insert(relative_path.clone(), (file.identity(), file.size()));
        }

        let discovered = discover_regular_files(&self.root, &self.discovery_rules, max_files)
            .map_err(|source| SourceRegistryError::DiscoveryFailed {
                source_id: self.descriptor.source_id.clone(),
                source,
            })?;
        for file in discovered {
            candidates
                .entry(file.relative_path)
                .or_insert((file.identity, file.size));
        }

        if candidates.len() > max_files {
            return Err(SourceRegistryError::TooManyFiles {
                source_id: self.descriptor.source_id.clone(),
                limit: max_files,
            });
        }

        Ok(candidates
            .into_iter()
            .enumerate()
            .map(
                |(index, (relative_path, (identity, size_at_snapshot)))| SourceFileSnapshot {
                    source_id: self.descriptor.source_id.clone(),
                    file_id: stable_file_id(&self.descriptor.source_id, &relative_path, index),
                    relative_path,
                    identity,
                    size_at_snapshot,
                },
            )
            .collect())
    }

    pub fn open_snapshot_file(
        &self,
        snapshot: &SourceFileSnapshot,
    ) -> Result<SafeFile, SourceRegistryError> {
        if snapshot.source_id != self.descriptor.source_id {
            return Err(SourceRegistryError::SnapshotSourceMismatch);
        }
        if !self.path_is_configured(&snapshot.relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }

        let file = self
            .root
            .open_regular_file(&snapshot.relative_path)
            .map_err(|source| SourceRegistryError::FileUnavailable {
                source_id: self.descriptor.source_id.clone(),
                source,
            })?;
        if file.identity() != snapshot.identity || file.size() < snapshot.size_at_snapshot {
            return Err(SourceRegistryError::FileChanged {
                source_id: self.descriptor.source_id.clone(),
                file_id: snapshot.file_id.clone(),
            });
        }
        Ok(file)
    }

    pub fn open_configured_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<SafeFile, SourceRegistryError> {
        let relative_path = relative_path.as_ref();
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }

        self.root
            .open_regular_file(relative_path)
            .map_err(|source| SourceRegistryError::FileUnavailable {
                source_id: self.descriptor.source_id.clone(),
                source,
            })
    }

    #[must_use]
    pub fn path_is_configured(&self, relative_path: &Path) -> bool {
        self.explicit_files
            .iter()
            .any(|candidate| candidate == relative_path)
            || self
                .directory_configs
                .iter()
                .any(|rule| directory_rule_allows(rule, relative_path))
    }
}

#[derive(Debug)]
pub struct SourceRegistry {
    sources: Vec<Arc<ConfiguredSource>>,
    by_id: HashMap<String, usize>,
    limits: LimitsConfig,
}

impl SourceRegistry {
    pub fn from_config(config: AppConfig) -> Result<Self, SourceRegistryError> {
        config.validate()?;
        let limits = config.limits.clone();
        let mut sources = Vec::new();
        let mut by_id = HashMap::new();

        for source_config in config.sources.into_iter().filter(|source| source.enabled) {
            let source_id = source_config.source_id.clone();
            let root = Arc::new(SafeRoot::open(&source_config.root).map_err(|source| {
                SourceRegistryError::RootUnavailable {
                    source_id: source_id.clone(),
                    source,
                }
            })?);

            let discovery_rules = source_config
                .directories
                .iter()
                .enumerate()
                .map(|(index, rule)| {
                    DirectoryDiscoveryRule::from_config(rule).map_err(|source| {
                        SourceRegistryError::DirectoryRuleInvalid {
                            source_id: source_id.clone(),
                            rule_index: index,
                            source,
                        }
                    })
                })
                .collect::<Result<Vec<_>, SourceRegistryError>>()?;

            let configured = Arc::new(ConfiguredSource {
                descriptor: SourceDescriptor {
                    source_id: source_config.source_id.clone(),
                    name: source_config.name,
                    description: source_config.description,
                    service: source_config.service,
                    environment: source_config.environment,
                    tags: source_config.tags,
                },
                root,
                explicit_files: source_config.files,
                directory_configs: source_config.directories,
                discovery_rules,
                timestamp_rule: source_config.timestamp_rule,
            });

            // Fail startup for unsafe explicit files, invalid directory roots,
            // or a discovery result beyond the absolute v1 hard limit.
            configured.snapshot_files(MAX_REGISTERED_FILES_PER_SOURCE)?;

            let index = sources.len();
            by_id.insert(source_id, index);
            sources.push(configured);
        }

        Ok(Self {
            sources,
            by_id,
            limits,
        })
    }

    #[must_use]
    pub fn list(&self) -> Vec<SourceDescriptor> {
        self.sources
            .iter()
            .map(|source| source.descriptor.clone())
            .collect()
    }

    #[must_use]
    pub fn get(&self, source_id: &str) -> Option<Arc<ConfiguredSource>> {
        self.by_id
            .get(source_id)
            .and_then(|index| self.sources.get(*index))
            .map(Arc::clone)
    }

    pub fn selected(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<Arc<ConfiguredSource>>, SourceRegistryError> {
        source_ids
            .iter()
            .map(|source_id| {
                self.get(source_id)
                    .ok_or_else(|| SourceRegistryError::UnknownSource(source_id.clone()))
            })
            .collect()
    }

    #[must_use]
    pub const fn limits(&self) -> &LimitsConfig {
        &self.limits
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.sources.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
    }
}

fn directory_rule_allows(rule: &DirectoryRule, relative_path: &Path) -> bool {
    let Some(file_name) = relative_path.file_name().and_then(|value| value.to_str()) else {
        return false;
    };
    if !rule
        .include_suffixes
        .iter()
        .any(|suffix| file_name.ends_with(suffix))
    {
        return false;
    }

    let remainder = if rule.path == Path::new(".") {
        relative_path
    } else {
        let Ok(remainder) = relative_path.strip_prefix(&rule.path) else {
            return false;
        };
        remainder
    };
    let component_count = remainder.components().count();
    component_count > 0 && (rule.recursive || component_count == 1)
}

fn stable_file_id(source_id: &str, relative_path: &Path, index: usize) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = FNV_OFFSET;
    for byte in source_id
        .as_bytes()
        .iter()
        .copied()
        .chain(std::iter::once(0))
        .chain(relative_path.as_os_str().as_bytes().iter().copied())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    format!("file_{hash:016x}_{index}")
}

#[derive(Debug, Error)]
pub enum SourceRegistryError {
    #[error(transparent)]
    InvalidConfiguration(#[from] ConfigValidationError),

    #[error("configured log source root is unavailable: {source_id}")]
    RootUnavailable {
        source_id: String,
        #[source]
        source: SafeOpenError,
    },

    #[error(
        "configured explicit log file is unavailable in source {source_id} at index {file_index}"
    )]
    ExplicitFileUnavailable {
        source_id: String,
        file_index: usize,
        #[source]
        source: SafeOpenError,
    },

    #[error("configured directory rule is invalid in source {source_id} at index {rule_index}")]
    DirectoryRuleInvalid {
        source_id: String,
        rule_index: usize,
        #[source]
        source: SourceDiscoveryError,
    },

    #[error("configured directory discovery failed for source {source_id}")]
    DiscoveryFailed {
        source_id: String,
        #[source]
        source: SourceDiscoveryError,
    },

    #[error("source {source_id} resolves more files than the limit {limit}")]
    TooManyFiles { source_id: String, limit: usize },

    #[error("unknown log source: {0}")]
    UnknownSource(String),

    #[error("file snapshot does not belong to this source")]
    SnapshotSourceMismatch,

    #[error("file path is not included by the configured source")]
    PathNotConfigured,

    #[error("configured file is temporarily unavailable for source {source_id}")]
    FileUnavailable {
        source_id: String,
        #[source]
        source: SafeOpenError,
    },

    #[error("configured file changed after the snapshot was created: {source_id}/{file_id}")]
    FileChanged { source_id: String, file_id: String },
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use crate::{CONFIG_VERSION, Encoding};

    use super::*;

    fn source(root: &Path, source_id: &str) -> crate::LogSourceConfig {
        crate::LogSourceConfig {
            source_id: source_id.to_owned(),
            name: source_id.to_owned(),
            description: String::new(),
            service: source_id.to_owned(),
            environment: "test".to_owned(),
            tags: Vec::new(),
            enabled: true,
            encoding: Encoding::Utf8,
            root: root.to_path_buf(),
            files: vec![PathBuf::from("application.log")],
            directories: Vec::new(),
            timestamp_rule: None,
        }
    }

    fn config(sources: Vec<crate::LogSourceConfig>) -> AppConfig {
        AppConfig {
            version: CONFIG_VERSION,
            sources,
            limits: LimitsConfig::default(),
        }
    }

    #[test]
    fn builds_registry_and_excludes_disabled_sources() {
        let enabled_root = tempdir().expect("enabled root should be created");
        fs::write(enabled_root.path().join("application.log"), "enabled\n")
            .expect("enabled fixture should be written");
        let mut disabled = source(Path::new("/definitely/missing"), "disabled");
        disabled.enabled = false;

        let registry = SourceRegistry::from_config(config(vec![
            source(enabled_root.path(), "payment-test"),
            disabled,
        ]))
        .expect("registry should build");

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.list()[0].source_id, "payment-test");
        assert!(registry.get("disabled").is_none());
    }

    #[test]
    fn snapshots_explicit_and_discovered_files_in_stable_order() {
        let root = tempdir().expect("source root should be created");
        fs::create_dir(root.path().join("archive")).expect("archive should be created");
        fs::write(root.path().join("application.log"), "current\n")
            .expect("current fixture should be written");
        fs::write(root.path().join("archive/application.log.1"), "old\n")
            .expect("archive fixture should be written");
        let mut source = source(root.path(), "payment-test");
        source.directories = vec![DirectoryRule {
            path: PathBuf::from("archive"),
            recursive: false,
            include_suffixes: vec![".log.1".to_owned()],
        }];
        let registry =
            SourceRegistry::from_config(config(vec![source])).expect("registry should build");
        let configured = registry.get("payment-test").expect("source should exist");

        let files = configured
            .snapshot_files(10)
            .expect("snapshot should succeed");
        assert_eq!(
            files
                .iter()
                .map(SourceFileSnapshot::display_name)
                .collect::<Vec<_>>(),
            vec!["application.log", "archive/application.log.1"]
        );
        assert_ne!(files[0].file_id(), files[1].file_id());
        assert!(!files[0].file_id().contains("application"));
    }

    #[test]
    fn fails_startup_for_missing_explicit_file() {
        let root = tempdir().expect("source root should be created");

        assert!(matches!(
            SourceRegistry::from_config(config(vec![source(root.path(), "payment-test")])),
            Err(SourceRegistryError::ExplicitFileUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_unknown_source_selection() {
        let root = tempdir().expect("source root should be created");
        fs::write(root.path().join("application.log"), "current\n")
            .expect("fixture should be written");
        let registry =
            SourceRegistry::from_config(config(vec![source(root.path(), "payment-test")]))
                .expect("registry should build");

        assert!(matches!(
            registry.selected(&["unknown".to_owned()]),
            Err(SourceRegistryError::UnknownSource(_))
        ));
    }

    #[test]
    fn detects_file_replacement_after_snapshot() {
        let root = tempdir().expect("source root should be created");
        let path = root.path().join("application.log");
        let rotated = root.path().join("application.log.1");
        fs::write(&path, "original\n").expect("fixture should be written");
        let registry =
            SourceRegistry::from_config(config(vec![source(root.path(), "payment-test")]))
                .expect("registry should build");
        let configured = registry.get("payment-test").expect("source should exist");
        let snapshot = configured
            .snapshot_files(10)
            .expect("snapshot should succeed")
            .remove(0);

        fs::rename(&path, &rotated).expect("original should rotate");
        fs::write(&path, "replacement\n").expect("replacement should be written");

        assert!(matches!(
            configured.open_snapshot_file(&snapshot),
            Err(SourceRegistryError::FileChanged { .. })
        ));
    }

    #[test]
    fn directory_rule_does_not_authorize_unconfigured_nested_path() {
        let root = tempdir().expect("source root should be created");
        fs::write(root.path().join("application.log"), "current\n")
            .expect("fixture should be written");
        fs::create_dir_all(root.path().join("archive/nested"))
            .expect("nested directory should be created");
        fs::write(root.path().join("archive/nested/secret.log"), "secret\n")
            .expect("nested fixture should be written");
        let mut source = source(root.path(), "payment-test");
        source.directories = vec![DirectoryRule {
            path: PathBuf::from("archive"),
            recursive: false,
            include_suffixes: vec![".log".to_owned()],
        }];
        let registry =
            SourceRegistry::from_config(config(vec![source])).expect("registry should build");
        let configured = registry.get("payment-test").expect("source should exist");

        assert!(matches!(
            configured.open_configured_file("archive/nested/secret.log"),
            Err(SourceRegistryError::PathNotConfigured)
        ));
    }

    #[test]
    fn symlink_discovery_entries_are_not_registered() {
        let root = tempdir().expect("source root should be created");
        let outside = tempdir().expect("outside directory should be created");
        fs::write(root.path().join("application.log"), "current\n")
            .expect("fixture should be written");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("outside fixture should be written");
        symlink(
            outside.path().join("secret.log"),
            root.path().join("linked.log"),
        )
        .expect("symlink should be created");
        let mut source = source(root.path(), "payment-test");
        source.directories = vec![DirectoryRule {
            path: PathBuf::from("."),
            recursive: false,
            include_suffixes: vec![".log".to_owned()],
        }];
        let registry =
            SourceRegistry::from_config(config(vec![source])).expect("registry should build");
        let configured = registry.get("payment-test").expect("source should exist");

        let files = configured
            .snapshot_files(10)
            .expect("snapshot should succeed");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].display_name(), "application.log");
    }
}
