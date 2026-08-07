use std::path::Path;

use crate::{FileIdentity, SafeFile, SourceRegistryError};

mod local;

pub(crate) use local::LocalBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendFileSnapshot {
    pub(crate) relative_path: std::path::PathBuf,
    pub(crate) identity: FileIdentity,
    pub(crate) size_at_snapshot: u64,
}

#[derive(Debug)]
pub(crate) enum SourceBackend {
    Local(LocalBackend),
}

impl SourceBackend {
    pub(crate) fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.snapshot_files(source_id, max_files),
        }
    }

    pub(crate) fn open_snapshot_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_snapshot: u64,
        file_id: &str,
    ) -> Result<SafeFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_snapshot_file(
                source_id,
                relative_path,
                identity,
                size_at_snapshot,
                file_id,
            ),
        }
    }

    pub(crate) fn open_configured_file(
        &self,
        source_id: &str,
        relative_path: &Path,
    ) -> Result<SafeFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_configured_file(source_id, relative_path),
        }
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        match self {
            Self::Local(backend) => backend.path_is_configured(relative_path),
        }
    }
}
