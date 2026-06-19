use std::{
    fs::File,
    os::fd::OwnedFd,
    path::{Component, Path},
};

use rustix::{
    fs::{FileType, Mode, OFlags, ResolveFlags, fstat, open, openat2},
    io::Errno,
};
use thiserror::Error;

const ROOT_OPEN_FLAGS: OFlags = OFlags::PATH
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW);
const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK);
const FILE_OPEN_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::CLOEXEC)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::NONBLOCK);
const RESOLVE_FLAGS: ResolveFlags = ResolveFlags::BENEATH
    .union(ResolveFlags::NO_SYMLINKS)
    .union(ResolveFlags::NO_MAGICLINKS);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileIdentity {
    pub device: u64,
    pub inode: u64,
}

#[derive(Debug)]
pub struct SafeFile {
    file: File,
    identity: FileIdentity,
    size: u64,
}

impl SafeFile {
    #[must_use]
    pub const fn identity(&self) -> FileIdentity {
        self.identity
    }

    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    #[must_use]
    pub const fn file(&self) -> &File {
        &self.file
    }

    #[must_use]
    pub fn into_file(self) -> File {
        self.file
    }
}

#[derive(Debug)]
pub struct SafeRoot {
    dirfd: OwnedFd,
}

impl SafeRoot {
    /// Open a configured log root without following a final symlink.
    ///
    /// The returned directory descriptor is retained so later file opens are
    /// resolved relative to the exact directory object that was approved.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, SafeOpenError> {
        let dirfd = open(path.as_ref(), ROOT_OPEN_FLAGS, Mode::empty())
            .map_err(|source| SafeOpenError::RootOpen { source })?;
        let stat = fstat(&dirfd).map_err(|source| SafeOpenError::RootOpen { source })?;

        if !FileType::from_raw_mode(stat.st_mode).is_dir() {
            return Err(SafeOpenError::RootNotDirectory);
        }

        Ok(Self { dirfd })
    }

    /// Open a regular file below this root using Linux `openat2()`.
    ///
    /// The kernel rejects parent traversal, absolute paths, all symlink
    /// components and procfs-style magic links while resolving the path.
    pub fn open_regular_file(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<SafeFile, SafeOpenError> {
        let relative_path = relative_path.as_ref();
        validate_relative_path(relative_path)?;

        let fd = openat2(
            &self.dirfd,
            relative_path,
            FILE_OPEN_FLAGS,
            Mode::empty(),
            RESOLVE_FLAGS,
        )
        .map_err(|source| SafeOpenError::FileOpen { source })?;
        let stat = fstat(&fd).map_err(|source| SafeOpenError::FileOpen { source })?;

        if !FileType::from_raw_mode(stat.st_mode).is_file() {
            return Err(SafeOpenError::NotRegularFile);
        }

        let device = stat.st_dev;
        let inode = stat.st_ino;
        let size = u64::try_from(stat.st_size).map_err(|_| SafeOpenError::MetadataOutOfRange)?;

        Ok(SafeFile {
            file: File::from(fd),
            identity: FileIdentity { device, inode },
            size,
        })
    }

    /// Open a directory below the configured root without following any
    /// symlink component. The returned descriptor is intended for bounded,
    /// fd-relative directory discovery inside the service.
    pub(crate) fn open_directory_fd(
        &self,
        relative_path: impl AsRef<Path>,
    ) -> Result<OwnedFd, SafeOpenError> {
        let relative_path = relative_path.as_ref();
        validate_directory_path(relative_path)?;

        let fd = openat2(
            &self.dirfd,
            relative_path,
            DIRECTORY_OPEN_FLAGS,
            Mode::empty(),
            RESOLVE_FLAGS,
        )
        .map_err(|source| SafeOpenError::DirectoryOpen { source })?;
        let stat = fstat(&fd).map_err(|source| SafeOpenError::DirectoryOpen { source })?;

        if !FileType::from_raw_mode(stat.st_mode).is_dir() {
            return Err(SafeOpenError::NotDirectory);
        }

        Ok(fd)
    }
}

fn validate_relative_path(path: &Path) -> Result<(), SafeOpenError> {
    let mut has_component = false;

    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => return Err(SafeOpenError::InvalidRelativePath),
        }
    }

    if !has_component {
        return Err(SafeOpenError::InvalidRelativePath);
    }

    Ok(())
}

fn validate_directory_path(path: &Path) -> Result<(), SafeOpenError> {
    if path == Path::new(".") {
        return Ok(());
    }
    validate_relative_path(path)
}

#[derive(Debug, Error)]
pub enum SafeOpenError {
    #[error("configured log root cannot be opened safely")]
    RootOpen {
        #[source]
        source: Errno,
    },

    #[error("configured log root is not a directory")]
    RootNotDirectory,

    #[error("log file path must be a normalized relative path")]
    InvalidRelativePath,

    #[error("log file cannot be opened safely")]
    FileOpen {
        #[source]
        source: Errno,
    },

    #[error("log directory cannot be opened safely")]
    DirectoryOpen {
        #[source]
        source: Errno,
    },

    #[error("opened object is not a regular file")]
    NotRegularFile,

    #[error("opened object is not a directory")]
    NotDirectory,

    #[error("file metadata cannot be represented by the service")]
    MetadataOutOfRange,
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        io::Read,
        os::unix::{fs::symlink, net::UnixListener},
    };

    use rustix::fs::{Mode, mkfifoat};
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn opens_regular_file_and_records_identity() {
        let root_dir = tempdir().expect("temporary root should be created");
        fs::write(root_dir.path().join("application.log"), "traceId=abc123\n")
            .expect("fixture should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");

        let safe_file = root
            .open_regular_file("application.log")
            .expect("regular file should open");
        assert!(safe_file.identity().inode > 0);
        assert_eq!(safe_file.size(), 15);

        let mut content = String::new();
        safe_file
            .into_file()
            .read_to_string(&mut content)
            .expect("file should be readable");
        assert_eq!(content, "traceId=abc123\n");
    }

    #[test]
    fn opens_root_and_nested_directories_without_symlinks() {
        let root_dir = tempdir().expect("temporary root should be created");
        fs::create_dir_all(root_dir.path().join("archive/day-1"))
            .expect("nested directories should be created");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");

        root.open_directory_fd(".")
            .expect("root directory should open for discovery");
        root.open_directory_fd("archive/day-1")
            .expect("nested directory should open for discovery");
    }

    #[test]
    fn rejects_parent_traversal_and_absolute_paths() {
        let root_dir = tempdir().expect("temporary root should be created");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");

        assert!(matches!(
            root.open_regular_file("../outside.log"),
            Err(SafeOpenError::InvalidRelativePath)
        ));
        assert!(matches!(
            root.open_regular_file("/etc/passwd"),
            Err(SafeOpenError::InvalidRelativePath)
        ));
        assert!(matches!(
            root.open_regular_file("./application.log"),
            Err(SafeOpenError::InvalidRelativePath)
        ));
        assert!(matches!(
            root.open_directory_fd("../outside"),
            Err(SafeOpenError::InvalidRelativePath)
        ));
    }

    #[test]
    fn rejects_final_and_intermediate_symlinks() {
        let root_dir = tempdir().expect("temporary root should be created");
        let outside_dir = tempdir().expect("outside directory should be created");
        fs::write(outside_dir.path().join("secret.log"), "secret\n")
            .expect("outside fixture should be written");

        symlink(
            outside_dir.path().join("secret.log"),
            root_dir.path().join("final-link.log"),
        )
        .expect("final symlink should be created");
        symlink(outside_dir.path(), root_dir.path().join("linked-dir"))
            .expect("directory symlink should be created");

        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        assert!(root.open_regular_file("final-link.log").is_err());
        assert!(root.open_regular_file("linked-dir/secret.log").is_err());
        assert!(root.open_directory_fd("linked-dir").is_err());
    }

    #[test]
    fn rejects_root_symlink() {
        let parent = tempdir().expect("temporary parent should be created");
        let actual_root = parent.path().join("actual");
        let linked_root = parent.path().join("linked");
        fs::create_dir(&actual_root).expect("actual root should be created");
        symlink(&actual_root, &linked_root).expect("root symlink should be created");

        assert!(SafeRoot::open(linked_root).is_err());
    }

    #[test]
    fn rejects_directory_fifo_and_socket() {
        let root_dir = tempdir().expect("temporary root should be created");
        fs::create_dir(root_dir.path().join("directory"))
            .expect("directory fixture should be created");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");
        mkfifoat(&root.dirfd, "pipe", Mode::RUSR | Mode::WUSR)
            .expect("fifo fixture should be created");
        let _listener = UnixListener::bind(root_dir.path().join("socket"))
            .expect("socket fixture should be created");

        assert!(matches!(
            root.open_regular_file("directory"),
            Err(SafeOpenError::NotRegularFile)
        ));
        assert!(root.open_regular_file("pipe").is_err());
        assert!(root.open_regular_file("socket").is_err());
    }

    #[test]
    fn rejects_file_replaced_by_symlink_before_open() {
        let root_dir = tempdir().expect("temporary root should be created");
        let outside_dir = tempdir().expect("outside directory should be created");
        let candidate = root_dir.path().join("candidate.log");
        let outside = outside_dir.path().join("outside.log");
        fs::write(&candidate, "original\n").expect("candidate should be written");
        fs::write(&outside, "outside\n").expect("outside file should be written");
        let root = SafeRoot::open(root_dir.path()).expect("root should open");

        fs::remove_file(&candidate).expect("candidate should be removed");
        symlink(&outside, &candidate).expect("replacement symlink should be created");

        assert!(root.open_regular_file("candidate.log").is_err());
    }
}
