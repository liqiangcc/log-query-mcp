use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Component, Path, PathBuf},
};

use rustix::{fs::Dir, io::Errno};
use thiserror::Error;

use crate::{DirectoryRule, SafeOpenError, SafeRoot};

pub const MAX_DIRECTORY_RULES_PER_SOURCE: usize = 64;
pub const MAX_DISCOVERY_SUFFIXES: usize = 32;
pub const MAX_DISCOVERY_SUFFIX_BYTES: usize = 128;
pub const MAX_DISCOVERY_ENTRIES: usize = 100_000;
pub const MAX_DISCOVERY_DIRECTORIES: usize = 10_000;
pub const MAX_DISCOVERED_FILES: usize = 10_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirectoryDiscoveryRule {
    directory: PathBuf,
    recursive: bool,
    suffixes: Vec<Vec<u8>>,
}

impl DirectoryDiscoveryRule {
    pub fn from_config(rule: &DirectoryRule) -> Result<Self, SourceDiscoveryError> {
        Self::new(
            rule.path.clone(),
            rule.recursive,
            rule.include_suffixes.clone(),
        )
    }

    pub fn new(
        directory: impl Into<PathBuf>,
        recursive: bool,
        include_suffixes: Vec<String>,
    ) -> Result<Self, SourceDiscoveryError> {
        let directory = directory.into();
        validate_directory_path(&directory)?;
        if include_suffixes.is_empty() || include_suffixes.len() > MAX_DISCOVERY_SUFFIXES {
            return Err(SourceDiscoveryError::InvalidRule(
                "include_suffixes count is outside the service limit",
            ));
        }

        let mut suffixes = Vec::with_capacity(include_suffixes.len());
        for suffix in include_suffixes {
            let bytes = suffix.into_bytes();
            if bytes.is_empty()
                || bytes.len() > MAX_DISCOVERY_SUFFIX_BYTES
                || bytes.first() != Some(&b'.')
                || bytes.contains(&b'/')
                || bytes.contains(&0)
                || bytes.contains(&b'\r')
                || bytes.contains(&b'\n')
            {
                return Err(SourceDiscoveryError::InvalidRule(
                    "include suffix is not a valid filename suffix",
                ));
            }
            if suffixes.contains(&bytes) {
                return Err(SourceDiscoveryError::InvalidRule(
                    "include_suffixes must not contain duplicates",
                ));
            }
            suffixes.push(bytes);
        }

        Ok(Self {
            directory,
            recursive,
            suffixes,
        })
    }

    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }

    #[must_use]
    pub const fn recursive(&self) -> bool {
        self.recursive
    }

    fn matches(&self, relative_path: &Path) -> bool {
        let Some(file_name) = relative_path.file_name() else {
            return false;
        };
        let bytes = file_name.as_bytes();
        self.suffixes.iter().any(|suffix| bytes.ends_with(suffix))
    }
}

pub fn discover_regular_files(
    root: &SafeRoot,
    rules: &[DirectoryDiscoveryRule],
    max_files: usize,
) -> Result<Vec<PathBuf>, SourceDiscoveryError> {
    if rules.is_empty() || rules.len() > MAX_DIRECTORY_RULES_PER_SOURCE {
        return Err(SourceDiscoveryError::InvalidRule(
            "directory rule count is outside the service limit",
        ));
    }
    if max_files == 0 || max_files > MAX_DISCOVERED_FILES {
        return Err(SourceDiscoveryError::TooManyFiles);
    }

    let mut files = Vec::new();
    let mut entries_seen = 0_usize;
    let mut directories_seen = 0_usize;

    for rule in rules {
        let mut pending = vec![rule.directory.clone()];
        while let Some(directory) = pending.pop() {
            directories_seen = directories_seen.saturating_add(1);
            if directories_seen > MAX_DISCOVERY_DIRECTORIES {
                return Err(SourceDiscoveryError::TooManyDirectories);
            }

            let fd = root.open_directory_fd(&directory)?;
            let mut dir = Dir::new(fd).map_err(SourceDiscoveryError::DirectoryRead)?;
            let mut entries = Vec::new();
            for entry in &mut dir {
                let entry = entry.map_err(SourceDiscoveryError::DirectoryRead)?;
                let name = entry.file_name().to_bytes();
                if name == b"." || name == b".." {
                    continue;
                }

                entries_seen = entries_seen.saturating_add(1);
                if entries_seen > MAX_DISCOVERY_ENTRIES {
                    return Err(SourceDiscoveryError::TooManyEntries);
                }
                entries.push((OsString::from_vec(name.to_vec()), entry.file_type()));
            }
            entries.sort_by(|left, right| left.0.cmp(&right.0));

            let mut child_directories = Vec::new();
            for (name, file_type) in entries {
                let relative_path = if directory == Path::new(".") {
                    PathBuf::from(name)
                } else {
                    directory.join(name)
                };

                if file_type.is_symlink() {
                    continue;
                }
                if file_type.is_dir() {
                    if rule.recursive {
                        root.open_directory_fd(&relative_path)?;
                        child_directories.push(relative_path);
                    }
                    continue;
                }
                if file_type.is_file() {
                    if rule.matches(&relative_path) {
                        root.open_regular_file(&relative_path)?;
                        push_file(&mut files, relative_path, max_files)?;
                    }
                    continue;
                }

                // Some filesystems report DT_UNKNOWN. Classification still
                // happens only through the openat2-based SafeRoot boundary.
                if rule.recursive && root.open_directory_fd(&relative_path).is_ok() {
                    child_directories.push(relative_path);
                } else if rule.matches(&relative_path)
                    && root.open_regular_file(&relative_path).is_ok()
                {
                    push_file(&mut files, relative_path, max_files)?;
                }
            }

            child_directories.sort();
            pending.extend(child_directories.into_iter().rev());
        }
    }

    files.sort();
    files.dedup();
    if files.len() > max_files {
        return Err(SourceDiscoveryError::TooManyFiles);
    }
    Ok(files)
}

fn push_file(
    files: &mut Vec<PathBuf>,
    path: PathBuf,
    max_files: usize,
) -> Result<(), SourceDiscoveryError> {
    files.push(path);
    if files.len() > max_files {
        Err(SourceDiscoveryError::TooManyFiles)
    } else {
        Ok(())
    }
}

fn validate_directory_path(path: &Path) -> Result<(), SourceDiscoveryError> {
    if path == Path::new(".") {
        return Ok(());
    }

    let mut has_component = false;
    for component in path.components() {
        match component {
            Component::Normal(_) => has_component = true,
            Component::Prefix(_)
            | Component::RootDir
            | Component::CurDir
            | Component::ParentDir => {
                return Err(SourceDiscoveryError::InvalidRule(
                    "directory path must be normalized and relative to its source root",
                ));
            }
        }
    }
    if !has_component {
        return Err(SourceDiscoveryError::InvalidRule(
            "directory path must identify a directory",
        ));
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum SourceDiscoveryError {
    #[error("invalid directory discovery rule: {0}")]
    InvalidRule(&'static str),

    #[error("directory discovery encountered too many entries")]
    TooManyEntries,

    #[error("directory discovery encountered too many directories")]
    TooManyDirectories,

    #[error("directory discovery matched too many files")]
    TooManyFiles,

    #[error("directory stream cannot be read safely")]
    DirectoryRead(#[source] Errno),

    #[error("directory discovery could not safely open a configured object")]
    SafeOpen(#[from] SafeOpenError),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn discovers_non_recursive_files_by_suffix() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "one\n")
            .expect("log fixture should be written");
        fs::write(directory.path().join("notes.txt"), "two\n")
            .expect("text fixture should be written");
        fs::create_dir(directory.path().join("archive"))
            .expect("archive directory should be created");
        fs::write(directory.path().join("archive/old.log"), "old\n")
            .expect("archive fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let rule = DirectoryDiscoveryRule::new(".", false, vec![".log".to_owned()])
            .expect("rule should be valid");

        let files =
            discover_regular_files(&root, &[rule], 10).expect("discovery should succeed");
        assert_eq!(files, vec![PathBuf::from("application.log")]);
    }

    #[test]
    fn recursively_discovers_rotation_files_without_following_symlinks() {
        let directory = tempdir().expect("temporary directory should be created");
        let outside = tempdir().expect("outside directory should be created");
        fs::create_dir_all(directory.path().join("logs/archive"))
            .expect("log directories should be created");
        fs::write(directory.path().join("logs/application.log"), "current\n")
            .expect("current log should be written");
        fs::write(
            directory.path().join("logs/archive/application.log.1"),
            "old\n",
        )
        .expect("rotated log should be written");
        fs::write(outside.path().join("secret.log"), "secret\n")
            .expect("outside file should be written");
        symlink(
            outside.path().join("secret.log"),
            directory.path().join("logs/linked.log"),
        )
        .expect("file symlink should be created");
        symlink(
            outside.path(),
            directory.path().join("logs/linked-directory"),
        )
        .expect("directory symlink should be created");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let rule = DirectoryDiscoveryRule::new(
            "logs",
            true,
            vec![".log".to_owned(), ".log.1".to_owned()],
        )
        .expect("rule should be valid");

        let files =
            discover_regular_files(&root, &[rule], 10).expect("discovery should succeed");
        assert_eq!(
            files,
            vec![
                PathBuf::from("logs/application.log"),
                PathBuf::from("logs/archive/application.log.1")
            ]
        );
    }

    #[test]
    fn deduplicates_overlapping_rules() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "one\n")
            .expect("log fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let first = DirectoryDiscoveryRule::new(".", false, vec![".log".to_owned()])
            .expect("rule should be valid");
        let second = DirectoryDiscoveryRule::new(".", false, vec!["ion.log".to_owned()]);
        assert!(second.is_err(), "suffixes must start with a dot");

        let overlapping =
            DirectoryDiscoveryRule::new(".", false, vec![".log".to_owned(), ".log".to_owned()]);
        assert!(overlapping.is_err());

        let files =
            discover_regular_files(&root, &[first], 10).expect("discovery should succeed");
        assert_eq!(files, vec![PathBuf::from("application.log")]);
    }

    #[test]
    fn enforces_file_limit_and_rule_validation() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("one.log"), "one\n")
            .expect("first fixture should be written");
        fs::write(directory.path().join("two.log"), "two\n")
            .expect("second fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let rule = DirectoryDiscoveryRule::new(".", false, vec![".log".to_owned()])
            .expect("rule should be valid");

        assert!(matches!(
            discover_regular_files(&root, &[rule], 1),
            Err(SourceDiscoveryError::TooManyFiles)
        ));
        assert!(DirectoryDiscoveryRule::new("../logs", false, vec![".log".to_owned()]).is_err());
        assert!(DirectoryDiscoveryRule::new(".", false, Vec::new()).is_err());
    }
}
