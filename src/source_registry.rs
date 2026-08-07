use std::{
    collections::HashMap,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

use crate::{
    AppConfig, AppConfigV2, BackendType, ConfigDocument, ConfigV2ValidationError,
    ConfigValidationError, FileIdentity, LimitsConfig, SafeFile, SafeOpenError,
    SourceDiscoveryError, TimestampRule,
    backend::{LocalBackend, SourceBackend},
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
    backend: SourceBackend,
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

        let snapshots = self
            .backend
            .snapshot_files(&self.descriptor.source_id, max_files)?;

        Ok(snapshots
            .into_iter()
            .enumerate()
            .map(|(index, snapshot)| SourceFileSnapshot {
                source_id: self.descriptor.source_id.clone(),
                file_id: stable_file_id(&self.descriptor.source_id, &snapshot.relative_path, index),
                relative_path: snapshot.relative_path,
                identity: snapshot.identity,
                size_at_snapshot: snapshot.size_at_snapshot,
            })
            .collect())
    }

    pub fn open_snapshot_file(
        &self,
        snapshot: &SourceFileSnapshot,
    ) -> Result<SafeFile, SourceRegistryError> {
        if snapshot.source_id != self.descriptor.source_id {
            return Err(SourceRegistryError::SnapshotSourceMismatch);
        }

        self.backend.open_snapshot_file(
            &self.descriptor.source_id,
            &snapshot.relative_path,
            snapshot.identity,
            snapshot.size_at_snapshot,
            &snapshot.file_id,
        )
    }

    pub fn open_configured_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<SafeFile, SourceRegistryError> {
        self.backend
            .open_configured_file(&self.descriptor.source_id, relative_path.as_ref())
    }

    #[must_use]
    pub fn path_is_configured(&self, relative_path: &Path) -> bool {
        self.backend.path_is_configured(relative_path)
    }
}

#[derive(Debug)]
pub struct SourceRegistry {
    sources: Vec<Arc<ConfiguredSource>>,
    by_id: HashMap<String, usize>,
    limits: LimitsConfig,
}

impl SourceRegistry {
    pub fn from_document(config: ConfigDocument) -> Result<Self, SourceRegistryError> {
        match config {
            ConfigDocument::V1(config) => Self::from_config(config),
            ConfigDocument::V2(config) => Self::from_config_v2(config),
        }
    }

    pub fn from_config_v2(config: AppConfigV2) -> Result<Self, SourceRegistryError> {
        config.validate()?;
        if let Some(source) = config
            .sources
            .iter()
            .find(|source| source.enabled && source.backend.backend_type == BackendType::Ssh)
        {
            return Err(SourceRegistryError::BackendUnavailable {
                source_id: source.source_id.clone(),
                backend: "ssh",
            });
        }
        Self::from_config(config.as_v1_shape())
    }

    pub fn from_config(config: AppConfig) -> Result<Self, SourceRegistryError> {
        config.validate()?;
        let limits = config.limits.clone();
        let mut sources = Vec::new();
        let mut by_id = HashMap::new();

        for source_config in config.sources.into_iter().filter(|source| source.enabled) {
            let source_id = source_config.source_id.clone();
            let backend =
                SourceBackend::Local(LocalBackend::from_config(&source_id, &source_config)?);

            let configured = Arc::new(ConfiguredSource {
                descriptor: SourceDescriptor {
                    source_id: source_config.source_id.clone(),
                    name: source_config.name,
                    description: source_config.description,
                    service: source_config.service,
                    environment: source_config.environment,
                    tags: source_config.tags,
                },
                backend,
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

    #[error(transparent)]
    InvalidV2Configuration(#[from] ConfigV2ValidationError),

    #[error("configured source backend is not available yet: {source_id}/{backend}")]
    BackendUnavailable {
        source_id: String,
        backend: &'static str,
    },

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

    use crate::{CONFIG_VERSION, DirectoryRule, Encoding};

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
