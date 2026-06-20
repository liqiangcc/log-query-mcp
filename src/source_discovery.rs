use std::{
    ffi::OsString,
    os::unix::ffi::{OsStrExt, OsStringExt},
    path::{Path, PathBuf},
};

use rustix::{fs::Dir, io::Errno};
use thiserror::Error;

use crate::{DirectoryRule, SafeOpenError, SafeRoot};

pub const MAX_DISCOVERY_ENTRIES: usize = 20_000;
pub const MAX_DISCOVERY_DIRECTORIES: usize = 1_000;

/// Discovers ordinary files under administrator-configured directory rules.
///
/// Every directory and candidate file is re-opened relative to `SafeRoot`, so
/// directory entries are never trusted as proof that an object is safe.
pub fn discover_regular_files(
    root: &SafeRoot,
    rules: &[DirectoryRule],
    max_files: usize,
) -> Result<Vec<PathBuf>, SourceDiscoveryError> {
    if rules.is_empty() {
        return Ok(Vec::new());
    }
    if max_files == 0 {
        return Err(SourceDiscoveryError::TooManyFiles);
    }

    let mut files = Vec::new();
    let mut entries_seen = 0_usize;
    let mut directories_seen = 0_usize;

    for rule in rules {
        let mut pending = vec![rule.path.clone()];
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
                let relative_path = join_relative(&directory, name);

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
                    if matches_suffix(&relative_path, &rule.include_suffixes) {
                        root.open_regular_file(&relative_path)?;
                        push_file(&mut files, relative_path, max_files)?;
                    }
                    continue;
                }

                // Some filesystems report DT_UNKNOWN. Classification still
                // happens through SafeRoot; special files and links are skipped.
                if rule.recursive && root.open_directory_fd(&relative_path).is_ok() {
                    child_directories.push(relative_path);
                } else if matches_suffix(&relative_path, &rule.include_suffixes)
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

fn join_relative(directory: &Path, name: OsString) -> PathBuf {
    if directory == Path::new(".") {
        PathBuf::from(name)
    } else {
        directory.join(name)
    }
}

fn matches_suffix(path: &Path, suffixes: &[String]) -> bool {
    let Some(file_name) = path.file_name() else {
        return false;
    };
    let bytes = file_name.as_bytes();
    suffixes
        .iter()
        .any(|suffix| bytes.ends_with(suffix.as_bytes()))
}

fn push_file(
    files: &mut Vec<PathBuf>,
    path: PathBuf,
    max_files: usize,
) -> Result<(), SourceDiscoveryError> {
    files.push(path);
    if files.len() > max_files {
        return Err(SourceDiscoveryError::TooManyFiles);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum SourceDiscoveryError {
    #[error("directory discovery encountered too many entries")]
    TooManyEntries,

    #[error("directory discovery encountered too many directories")]
    TooManyDirectories,

    #[error("directory discovery matched too many files")]
    TooManyFiles,

    #[error("configured directory cannot be read safely")]
    DirectoryRead(#[source] Errno),

    #[error("configured object cannot be opened safely during discovery")]
    SafeOpen(#[from] SafeOpenError),
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use tempfile::tempdir;

    use super::*;

    fn rule(path: &str, recursive: bool, suffixes: &[&str]) -> DirectoryRule {
        DirectoryRule {
            path: PathBuf::from(path),
            recursive,
            include_suffixes: suffixes.iter().map(ToString::to_string).collect(),
        }
    }

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

        let files = discover_regular_files(&root, &[rule(".", false, &[".log"])], 10)
            .expect("directory discovery should succeed");
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

        let files = discover_regular_files(
            &root,
            &[rule("logs", true, &[".log", ".log.1"])],
            10,
        )
        .expect("recursive discovery should succeed");
        assert_eq!(
            files,
            vec![
                PathBuf::from("logs/application.log"),
                PathBuf::from("logs/archive/application.log.1")
            ]
        );
    }

    #[test]
    fn deduplicates_overlapping_rules_and_returns_stable_order() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("b.log"), "b\n").expect("fixture should be written");
        fs::write(directory.path().join("a.log"), "a\n").expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");

        let files = discover_regular_files(
            &root,
            &[
                rule(".", false, &[".log"]),
                rule(".", false, &["a.log", ".log"]),
            ],
            10,
        )
        .expect("directory discovery should succeed");
        assert_eq!(files, vec![PathBuf::from("a.log"), PathBuf::from("b.log")]);
    }

    #[test]
    fn enforces_file_limit() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("one.log"), "one\n")
            .expect("fixture should be written");
        fs::write(directory.path().join("two.log"), "two\n")
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");

        assert!(matches!(
            discover_regular_files(&root, &[rule(".", false, &[".log"])], 1),
            Err(SourceDiscoveryError::TooManyFiles)
        ));
    }
}
