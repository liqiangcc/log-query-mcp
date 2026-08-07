use std::{
    collections::BTreeMap,
    fmt,
    path::{Component, Path, PathBuf},
};

use tokio::task::JoinSet;
use uuid::Uuid;

use crate::{
    CacheStore, FileIdentity, GenerationId, GenerationPin, LogSourceConfigV2, RemoteSyncTarget,
    SourceRegistryError, SyncEngine,
    transport::{RemoteFileType, SshConnectionManager, SshReadTransport},
};

use super::{BackendFileSnapshot, SnapshotFile};

pub(crate) struct RemoteBackend {
    source: LogSourceConfigV2,
    cache: CacheStore,
    sync: SyncEngine,
    connections: SshConnectionManager,
    max_remote_files: usize,
}

impl fmt::Debug for RemoteBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RemoteBackend")
            .field("source_id", &self.source.source_id)
            .field("max_remote_files", &self.max_remote_files)
            .finish_non_exhaustive()
    }
}

impl RemoteBackend {
    pub(crate) fn new(
        source: LogSourceConfigV2,
        cache: CacheStore,
        sync: SyncEngine,
        connections: SshConnectionManager,
        max_remote_files: usize,
    ) -> Result<Self, SourceRegistryError> {
        if max_remote_files == 0 {
            return Err(SourceRegistryError::RemoteConfigurationInvalid);
        }
        if source.directories.iter().any(|rule| rule.recursive) {
            return Err(SourceRegistryError::RemoteRecursiveDiscoveryUnsupported {
                source_id: source.source_id.clone(),
            });
        }
        Ok(Self {
            source,
            cache,
            sync,
            connections,
            max_remote_files,
        })
    }

    pub(crate) async fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        let limit = max_files.min(self.max_remote_files);
        if limit == 0 {
            return Err(SourceRegistryError::TooManyFiles {
                source_id: source_id.to_owned(),
                limit,
            });
        }
        let identifiers = self.discover_identifiers(source_id, limit).await?;
        let mut tasks = JoinSet::new();
        for identifier in identifiers {
            let sync = self.sync.clone();
            let source = self.source.clone();
            tasks.spawn(async move {
                let target = RemoteSyncTarget::from_source(&source, identifier.clone())?;
                let outcome = sync.sync(&target).await?;
                Ok::<_, crate::SyncError>((identifier, outcome))
            });
        }

        let mut synchronized = BTreeMap::new();
        while let Some(joined) = tasks.join_next().await {
            let (identifier, outcome) = match joined {
                Ok(Ok(value)) => value,
                Ok(Err(source)) => {
                    tasks.abort_all();
                    return Err(SourceRegistryError::RemoteSync {
                        source_id: source_id.to_owned(),
                        source,
                    });
                }
                Err(source) => {
                    tasks.abort_all();
                    return Err(SourceRegistryError::RemoteTaskJoin {
                        source_id: source_id.to_owned(),
                        source,
                    });
                }
            };
            synchronized.insert(identifier, outcome);
        }

        synchronized
            .into_iter()
            .enumerate()
            .map(|(index, (identifier, outcome))| {
                let pin = self
                    .cache
                    .lease_generation(source_id, &identifier, &outcome.generation)
                    .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                        source_id: source_id.to_owned(),
                        file_id: format!("remote_{index}"),
                        source,
                    })?;
                let relative_path = PathBuf::from(&identifier);
                Ok(BackendFileSnapshot {
                    relative_path,
                    identity: generation_identity(pin.generation_id())?,
                    size_at_snapshot: outcome.cached_range.len(),
                    coverage: Some(outcome.coverage),
                    generation_pin: Some(pin),
                })
            })
            .collect()
    }

    pub(crate) fn open_snapshot_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_snapshot: u64,
        file_id: &str,
        generation_pin: Option<&GenerationPin>,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }
        let pin = generation_pin.ok_or(SourceRegistryError::RemoteSnapshotMissingPin)?;
        if generation_identity(pin.generation_id())? != identity
            || pin.record().data_len != size_at_snapshot
        {
            return Err(SourceRegistryError::FileChanged {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
            });
        }
        let identifier = remote_identifier(relative_path)?;
        let reader = self
            .cache
            .pin_generation(source_id, &identifier, pin.generation_id())
            .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
                source,
            })?;
        Ok(SnapshotFile::Remote {
            reader,
            identity,
            size: size_at_snapshot,
        })
    }

    pub(crate) fn open_referenced_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_match: u64,
        file_id: &str,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        if !self.path_is_configured(relative_path) {
            return Err(SourceRegistryError::PathNotConfigured);
        }
        let generation = identity_generation(identity)?;
        let identifier = remote_identifier(relative_path)?;
        let reader = self
            .cache
            .pin_generation(source_id, &identifier, &generation)
            .map_err(|source| SourceRegistryError::CachedGenerationUnavailable {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
                source,
            })?;
        if reader.record().data_len < size_at_match {
            return Err(SourceRegistryError::FileChanged {
                source_id: source_id.to_owned(),
                file_id: file_id.to_owned(),
            });
        }
        let size = reader.record().data_len;
        Ok(SnapshotFile::Remote {
            reader,
            identity,
            size,
        })
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        self.source
            .files
            .iter()
            .any(|candidate| candidate == relative_path)
            || self
                .source
                .directories
                .iter()
                .any(|rule| directory_rule_allows(rule, relative_path))
    }

    async fn discover_identifiers(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<String>, SourceRegistryError> {
        let connection_id = self
            .source
            .backend
            .connection_id
            .as_deref()
            .ok_or(SourceRegistryError::RemoteConfigurationInvalid)?;
        let reader = self
            .connections
            .open_reader(connection_id)
            .await
            .map_err(|source| SourceRegistryError::RemoteTransport {
                source_id: source_id.to_owned(),
                source,
            })?;
        let result = self
            .discover_with_reader(source_id, max_files, &reader)
            .await;
        let _ = reader.close().await;
        result
    }

    async fn discover_with_reader(
        &self,
        source_id: &str,
        max_files: usize,
        reader: &SshReadTransport,
    ) -> Result<Vec<String>, SourceRegistryError> {
        let mut candidates = BTreeMap::<String, ()>::new();
        for (index, path) in self.source.files.iter().enumerate() {
            let identifier = remote_identifier(path)?;
            let remote_path = configured_remote_path(&self.source.root, path)?;
            let metadata = reader.lstat(&remote_path).await.map_err(|source| {
                SourceRegistryError::RemoteTransport {
                    source_id: source_id.to_owned(),
                    source,
                }
            })?;
            if metadata.file_type != RemoteFileType::Regular {
                return Err(SourceRegistryError::RemoteExplicitFileNotRegular {
                    source_id: source_id.to_owned(),
                    file_index: index,
                });
            }
            candidates.insert(identifier, ());
            ensure_file_limit(source_id, candidates.len(), max_files)?;
        }

        for rule in &self.source.directories {
            let directory = configured_remote_path(&self.source.root, &rule.path)?;
            let entries = reader.read_dir(&directory).await.map_err(|source| {
                SourceRegistryError::RemoteTransport {
                    source_id: source_id.to_owned(),
                    source,
                }
            })?;
            if entries.len() > self.max_remote_files {
                return Err(SourceRegistryError::TooManyFiles {
                    source_id: source_id.to_owned(),
                    limit: self.max_remote_files,
                });
            }
            for entry in entries {
                if !valid_remote_name(&entry.file_name)
                    || !rule
                        .include_suffixes
                        .iter()
                        .any(|suffix| entry.file_name.ends_with(suffix))
                {
                    continue;
                }
                let relative_path = if rule.path == Path::new(".") {
                    PathBuf::from(&entry.file_name)
                } else {
                    rule.path.join(&entry.file_name)
                };
                let remote_path = configured_remote_path(&self.source.root, &relative_path)?;
                let metadata = reader.lstat(&remote_path).await.map_err(|source| {
                    SourceRegistryError::RemoteTransport {
                        source_id: source_id.to_owned(),
                        source,
                    }
                })?;
                if metadata.file_type != RemoteFileType::Regular {
                    continue;
                }
                candidates.insert(remote_identifier(&relative_path)?, ());
                ensure_file_limit(source_id, candidates.len(), max_files)?;
            }
        }
        Ok(candidates.into_keys().collect())
    }
}

fn ensure_file_limit(
    source_id: &str,
    count: usize,
    limit: usize,
) -> Result<(), SourceRegistryError> {
    if count > limit {
        Err(SourceRegistryError::TooManyFiles {
            source_id: source_id.to_owned(),
            limit,
        })
    } else {
        Ok(())
    }
}

fn valid_remote_name(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
}

fn remote_identifier(path: &Path) -> Result<String, SourceRegistryError> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => {
                let value = value
                    .to_str()
                    .ok_or(SourceRegistryError::RemotePathInvalid)?;
                if !valid_remote_name(value) {
                    return Err(SourceRegistryError::RemotePathInvalid);
                }
                parts.push(value);
            }
            _ => return Err(SourceRegistryError::RemotePathInvalid),
        }
    }
    if parts.is_empty() {
        return Err(SourceRegistryError::RemotePathInvalid);
    }
    Ok(parts.join("/"))
}

fn configured_remote_path(root: &Path, relative: &Path) -> Result<String, SourceRegistryError> {
    let root = root
        .to_str()
        .ok_or(SourceRegistryError::RemotePathInvalid)?;
    if root.is_empty() || !root.starts_with('/') || root.chars().any(char::is_control) {
        return Err(SourceRegistryError::RemotePathInvalid);
    }
    let identifier = if relative == Path::new(".") {
        String::new()
    } else {
        remote_identifier(relative)?
    };
    let root = if root == "/" {
        ""
    } else {
        root.trim_end_matches('/')
    };
    if identifier.is_empty() {
        Ok(if root.is_empty() {
            "/".to_owned()
        } else {
            root.to_owned()
        })
    } else {
        Ok(format!("{root}/{identifier}"))
    }
}

fn directory_rule_allows(rule: &crate::DirectoryRule, relative_path: &Path) -> bool {
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
    remainder.components().count() == 1
}

fn generation_identity(generation: &GenerationId) -> Result<FileIdentity, SourceRegistryError> {
    let uuid =
        Uuid::parse_str(generation.as_str()).map_err(|_| SourceRegistryError::RemotePathInvalid)?;
    let value = uuid.as_u128();
    Ok(FileIdentity {
        device: (value >> 64) as u64,
        inode: value as u64,
    })
}

fn identity_generation(identity: FileIdentity) -> Result<GenerationId, SourceRegistryError> {
    let value = (u128::from(identity.device) << 64) | u128::from(identity.inode);
    let uuid = Uuid::from_u128(value);
    GenerationId::parse(uuid.simple().to_string())
        .map_err(|_| SourceRegistryError::RemotePathInvalid)
}
