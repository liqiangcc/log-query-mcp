use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Take},
};

use thiserror::Error;

use crate::{SafeRoot, SearchCursorData, SearchCursorFileError, open_cursor_file};

/// Reader constrained to the exact file snapshot captured by a search cursor.
///
/// Even when the underlying log file has grown, this reader reaches EOF at
/// `file_size_at_snapshot`, so a continuation page cannot include records that
/// were appended after the original query began.
#[derive(Debug)]
pub struct CursorSnapshotReader {
    inner: Take<File>,
    start_offset: u64,
    snapshot_end_offset: u64,
}

impl CursorSnapshotReader {
    #[must_use]
    pub const fn start_offset(&self) -> u64 {
        self.start_offset
    }

    #[must_use]
    pub const fn snapshot_end_offset(&self) -> u64 {
        self.snapshot_end_offset
    }

    #[must_use]
    pub fn remaining_bytes(&self) -> u64 {
        self.inner.limit()
    }

    #[must_use]
    pub fn into_inner(self) -> Take<File> {
        self.inner
    }
}

impl Read for CursorSnapshotReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.inner.read(buffer)
    }
}

/// Reopens the current cursor file, verifies that the continuation offset is a
/// log-line boundary, seeks to that offset and caps all reads at the original
/// snapshot size.
pub fn open_cursor_snapshot_reader(
    root: &SafeRoot,
    cursor: &SearchCursorData,
) -> Result<CursorSnapshotReader, CursorSnapshotError> {
    let safe_file = open_cursor_file(root, cursor)?;
    let candidate = cursor.current_candidate();
    let start_offset = cursor.next_byte_offset;
    let snapshot_end_offset = candidate.file_size_at_snapshot;

    if start_offset >= snapshot_end_offset {
        return Err(CursorSnapshotError::NoRemainingSnapshot);
    }

    let mut file = safe_file.into_file();
    if start_offset > 0 {
        file.seek(SeekFrom::Start(start_offset - 1))?;
        let mut previous = [0_u8; 1];
        file.read_exact(&mut previous)?;
        if previous[0] != b'\n' {
            return Err(CursorSnapshotError::InvalidLineBoundary);
        }
    }

    file.seek(SeekFrom::Start(start_offset))?;
    let snapshot_bytes = snapshot_end_offset - start_offset;

    Ok(CursorSnapshotReader {
        inner: file.take(snapshot_bytes),
        start_offset,
        snapshot_end_offset,
    })
}

#[derive(Debug, Error)]
pub enum CursorSnapshotError {
    #[error("cursor log file cannot be reopened from its stable snapshot")]
    CursorFile(#[from] SearchCursorFileError),

    #[error("cursor continuation has no unread bytes in the captured snapshot")]
    NoRemainingSnapshot,

    #[error("cursor continuation offset is not positioned at a log-line boundary")]
    InvalidLineBoundary,

    #[error("cursor snapshot seek or read failed")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read};

    use tempfile::tempdir;

    use crate::{CursorCandidateFile, ResultOrder, SafeRoot, SearchCursorQuery};

    use super::*;

    fn cursor_for_file(
        root: &SafeRoot,
        relative_path: &str,
        next_byte_offset: u64,
        next_line_number: u64,
    ) -> SearchCursorData {
        let safe_file = root
            .open_regular_file(relative_path)
            .expect("fixture should open safely");
        SearchCursorData {
            query: SearchCursorQuery {
                source_ids: vec!["payment-test".to_owned()],
                keyword: "traceId=abc123".to_owned(),
                case_sensitive: false,
                start_time: None,
                end_time: None,
                order: ResultOrder::OldestFirst,
                max_results: 50,
            },
            candidates: vec![CursorCandidateFile {
                source_id: "payment-test".to_owned(),
                relative_path: relative_path.into(),
                file_identity: safe_file.identity(),
                file_size_at_snapshot: safe_file.size(),
            }],
            next_candidate_index: 0,
            next_byte_offset,
            next_line_number,
            files_scanned: 0,
            bytes_scanned: next_byte_offset,
            results_returned: 1,
        }
    }

    #[test]
    fn reads_only_unread_bytes_from_original_snapshot() {
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("application.log");
        let first_line = b"first traceId=abc123\n";
        let second_line = b"second traceId=abc123\n";
        let mut original = first_line.to_vec();
        original.extend_from_slice(second_line);
        fs::write(&path, &original).expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let cursor = cursor_for_file(
            &root,
            "application.log",
            u64::try_from(first_line.len()).expect("line length should fit"),
            2,
        );

        let mut appended = original.clone();
        appended.extend_from_slice(b"third traceId=abc123\n");
        fs::write(&path, appended).expect("fixture should be appended");

        let mut reader =
            open_cursor_snapshot_reader(&root, &cursor).expect("snapshot reader should open");
        let mut observed = Vec::new();
        reader
            .read_to_end(&mut observed)
            .expect("snapshot should be readable");

        assert_eq!(observed, second_line);
        assert_eq!(reader.remaining_bytes(), 0);
    }

    #[test]
    fn rejects_offset_inside_a_log_line() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(
            directory.path().join("application.log"),
            b"first traceId=abc123\nsecond line\n",
        )
        .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let cursor = cursor_for_file(&root, "application.log", 3, 1);

        assert!(matches!(
            open_cursor_snapshot_reader(&root, &cursor),
            Err(CursorSnapshotError::InvalidLineBoundary)
        ));
    }

    #[test]
    fn rejects_cursor_positioned_at_snapshot_eof() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(
            directory.path().join("application.log"),
            b"only traceId=abc123\n",
        )
        .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let safe_file = root
            .open_regular_file("application.log")
            .expect("fixture should open safely");
        let cursor = cursor_for_file(&root, "application.log", safe_file.size(), 2);

        assert!(matches!(
            open_cursor_snapshot_reader(&root, &cursor),
            Err(CursorSnapshotError::NoRemainingSnapshot)
        ));
    }

    #[test]
    fn starts_at_file_beginning_without_boundary_probe() {
        let directory = tempdir().expect("temporary directory should be created");
        let content = b"first traceId=abc123\nsecond line\n";
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let cursor = cursor_for_file(&root, "application.log", 0, 1);

        let mut reader =
            open_cursor_snapshot_reader(&root, &cursor).expect("snapshot reader should open");
        let mut observed = Vec::new();
        reader
            .read_to_end(&mut observed)
            .expect("snapshot should be readable");

        assert_eq!(observed, content);
    }
}
