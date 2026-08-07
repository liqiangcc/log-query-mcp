use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    DirectoryDiscoveryRule, DirectoryRule, FileIdentity, LogSourceConfig, SafeFile, SafeRoot,
    SourceRegistryError, discover_regular_files,
};

use super::BackendFileSnapshot;

/// Local log access backend that preserves the v1 `SafeRoot`/`openat2()` security boundary.
#[derive(Debug)]
pub(crate) struct LocalBackend {
    root: Arc<SafeRoot>,
    explicit_files: Vec<PathBuf>,
    directory_configs: Vec<DirectoryRule>,
    discovery_rules: Vec<DirectoryDiscoveryRule>,
}

impl LocalBackend {
    pub(crate) fn from_config(
        source_id: &str,
        source_config: &LogSourceConfig,
    ) -> Result<Self, SourceRegistryError> {
        let root = Arc::new(SafeRoot::open(&source_config.root).map_err(|source| {
            SourceRegistryError::RootUnavailable {
                source_id: source_id.to_owned(),
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
                        source_id: source_id.to_owned(),
                        rule_index: index,
                        source,
                    }
                })
            })
            .collect::<Result<Vec<_>, SourceRegistryError>>()?;

        Ok(Self {
            root,
            explicit_files: source_config.files.clone(),
            directory_configs: source_config.directories.clone(),
            discovery_rules,
        })
    }

    pub(crate) fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        let mut candidates = BTreeMap::<PathBuf, (FileIdentity, u64)>::new();
        for (index, relative_path) in self.explicit_files.iter().enumerate() {
            let file = self
                .root
                .open_regular_file(relative_path)
                .map_err(|source| SourceRegistryError::ExplicitFileUnavailable {
                    source_id: source_id.to_owned(),
                    file_index: index,
                    source,
                })?;
            candidates.insert(relative_path.clone(), (file.identity(), file.size()));
        }

        let discovered = discover_regular_files(&self.root, &self.discovery_rules, max_files)
            .map_err(|source| SourceRegistryError::DiscoveryFailed {
                source_id: source_id.to_owned(),
                source,
            })?;
        for file in discovered {
            candidates
                .entry(file.relative_path)
                .or_insert((file.identity, file.size));
        }

        if candidates.len() > max_files {
            return Err(SourceRegistryError::TooManyFiles {
                source_id: source_id.to_owned(),
                limit: max_files,
            });
        }

        Ok(candidates
            .into_iter()
            .map(
                |(relative_path, (identity, size_at_snapshot))| BackendFileSnapshot {
                    relative_path,
                    identity,
                    size_at_snapshot,
                },
            )
            .collect())
    }

    pub(crate) fn open_snapshot_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_snapshot: u64,
        file_id: &str,
    ) -> Result<SafeFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }

        let file = self
            .root
            .open_regular_file(relative_path)
            .map_err(|source| SourceRegistryError::FileUnavailable {
                source_id: source_id.to_owned(),
                source,
            })?;
        if file.identity() != identity || file.size() < size_at_snapshot {
            return Err(SourceRegistryError::FileChanged {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
            });
        }
        Ok(file)
    }

    pub(crate) fn open_configured_file(
        &self,
        source_id: &str,
        relative_path: &Path,
    ) -> Result<SafeFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }

        self.root
            .open_regular_file(relative_path)
            .map_err(|source| SourceRegistryError::FileUnavailable {
                source_id: source_id.to_owned(),
                source,
            })
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        self.explicit_files
            .iter()
            .any(|candidate| candidate == relative_path)
            || self
                .directory_configs
                .iter()
                .any(|rule| directory_rule_allows(rule, relative_path))
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
