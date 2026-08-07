use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use crate::{FileIdentity, GenerationPin, PinnedGeneration, SafeFile, SourceRegistryError};

mod local;
mod remote;

pub(crate) use local::LocalBackend;
pub(crate) use remote::RemoteBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BackendFileSnapshot {
    pub(crate) relative_path: std::path::PathBuf,
    pub(crate) identity: FileIdentity,
    pub(crate) size_at_snapshot: u64,
    pub(crate) coverage: Option<crate::CacheCoverage>,
    pub(crate) generation_pin: Option<GenerationPin>,
}

pub enum SnapshotFile {
    Local(SafeFile),
    Remote {
        reader: PinnedGeneration,
        identity: FileIdentity,
        size: u64,
    },
}

impl std::fmt::Debug for SnapshotFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Local(file) => formatter
                .debug_struct("SnapshotFile::Local")
                .field("identity", &file.identity())
                .field("size", &file.size())
                .finish(),
            Self::Remote { identity, size, .. } => formatter
                .debug_struct("SnapshotFile::Remote")
                .field("identity", identity)
                .field("size", size)
                .finish(),
        }
    }
}

impl SnapshotFile {
    #[must_use]
    pub fn identity(&self) -> FileIdentity {
        match self {
            Self::Local(file) => file.identity(),
            Self::Remote { identity, .. } => *identity,
        }
    }

    #[must_use]
    pub fn size(&self) -> u64 {
        match self {
            Self::Local(file) => file.size(),
            Self::Remote { size, .. } => *size,
        }
    }

    #[must_use]
    pub fn into_file(self) -> Self {
        self
    }
}

impl Read for SnapshotFile {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::Local(file) => file.file().read(buffer),
            Self::Remote { reader, .. } => reader.read(buffer),
        }
    }
}

impl Seek for SnapshotFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::Local(file) => file.file().seek(position),
            Self::Remote { reader, .. } => reader.seek(position),
        }
    }
}

#[derive(Debug)]
pub(crate) enum SourceBackend {
    Local(LocalBackend),
    Remote(RemoteBackend),
}

impl SourceBackend {
    pub(crate) fn startup_validate(&self, source_id: &str) -> Result<(), SourceRegistryError> {
        match self {
            Self::Local(backend) => {
                backend.snapshot_files(source_id, crate::MAX_REGISTERED_FILES_PER_SOURCE)?;
                Ok(())
            }
            Self::Remote(_) => Ok(()),
        }
    }

    pub(crate) fn snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.snapshot_files(source_id, max_files),
            Self::Remote(_) => Err(SourceRegistryError::AsyncBackendRequired),
        }
    }

    pub(crate) async fn query_snapshot_files(
        &self,
        source_id: &str,
        max_files: usize,
    ) -> Result<Vec<BackendFileSnapshot>, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.snapshot_files(source_id, max_files),
            Self::Remote(backend) => backend.snapshot_files(source_id, max_files).await,
        }
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
        match self {
            Self::Local(backend) => backend.open_snapshot_file(
                source_id,
                relative_path,
                identity,
                size_at_snapshot,
                file_id,
            ),
            Self::Remote(backend) => backend.open_snapshot_file(
                source_id,
                relative_path,
                identity,
                size_at_snapshot,
                file_id,
                generation_pin,
            ),
        }
    }

    pub(crate) fn open_referenced_file(
        &self,
        source_id: &str,
        relative_path: &Path,
        identity: FileIdentity,
        size_at_match: u64,
        file_id: &str,
    ) -> Result<SnapshotFile, SourceRegistryError> {
        match self {
            Self::Local(backend) => backend.open_referenced_file(
                source_id,
                relative_path,
                identity,
                size_at_match,
                file_id,
            ),
            Self::Remote(backend) => backend.open_referenced_file(
                source_id,
                relative_path,
                identity,
                size_at_match,
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
            Self::Remote(_) => Err(SourceRegistryError::AsyncBackendRequired),
        }
    }

    pub(crate) fn path_is_configured(&self, relative_path: &Path) -> bool {
        match self {
            Self::Local(backend) => backend.path_is_configured(relative_path),
            Self::Remote(backend) => backend.path_is_configured(relative_path),
        }
    }
}
