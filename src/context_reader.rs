use std::io::{ErrorKind, Read, Seek, SeekFrom};

use thiserror::Error;

use crate::{
    MAX_CONTEXT_LINES_PER_SIDE, MAX_LINE_PREVIEW_BYTES, MAX_READ_BUFFER_BYTES,
    MAX_RETURNED_CONTENT_BYTES, MatchReferenceData, MatchReferenceFileError, SafeRoot,
    open_referenced_file,
};

pub const MAX_CONTEXT_BACKTRACK_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CONTEXT_FORWARD_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextReadLimits {
    pub max_backtrack_bytes: u64,
    pub max_forward_bytes: u64,
    pub max_line_bytes: usize,
    pub max_returned_content_bytes: usize,
    pub read_buffer_bytes: usize,
}

impl Default for ContextReadLimits {
    fn default() -> Self {
        Self {
            max_backtrack_bytes: 4 * 1024 * 1024,
            max_forward_bytes: 4 * 1024 * 1024,
            max_line_bytes: 16 * 1024,
            max_returned_content_bytes: 1024 * 1024,
            read_buffer_bytes: 64 * 1024,
        }
    }
}

impl ContextReadLimits {
    fn validate(self, before_lines: usize, after_lines: usize) -> Result<(), ContextReadError> {
        if before_lines > MAX_CONTEXT_LINES_PER_SIDE || after_lines > MAX_CONTEXT_LINES_PER_SIDE {
            return Err(ContextReadError::InvalidRequest(
                "context line count exceeds the server limit",
            ));
        }
        if self.max_backtrack_bytes == 0 || self.max_backtrack_bytes > MAX_CONTEXT_BACKTRACK_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "max_backtrack_bytes is outside the server limit",
            ));
        }
        if self.max_forward_bytes == 0 || self.max_forward_bytes > MAX_CONTEXT_FORWARD_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "max_forward_bytes is outside the server limit",
            ));
        }
        if self.max_line_bytes == 0 || self.max_line_bytes > MAX_LINE_PREVIEW_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "max_line_bytes is outside the server limit",
            ));
        }
        if self.max_returned_content_bytes == 0
            || self.max_returned_content_bytes > MAX_RETURNED_CONTENT_BYTES
        {
            return Err(ContextReadError::InvalidLimits(
                "max_returned_content_bytes is outside the server limit",
            ));
        }
        if self.read_buffer_bytes == 0 || self.read_buffer_bytes > MAX_READ_BUFFER_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "read_buffer_bytes is outside the server limit",
            ));
        }

        let guaranteed_match_budget = self
            .max_line_bytes
            .checked_mul(before_lines.saturating_add(1))
            .ok_or(ContextReadError::InvalidLimits(
                "context content budget overflows",
            ))?;
        if guaranteed_match_budget > self.max_returned_content_bytes {
            return Err(ContextReadError::InvalidLimits(
                "returned content budget cannot retain all requested preceding lines and the match line",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextReadLine {
    pub line_number: u64,
    pub content: String,
    pub content_truncated: bool,
    pub content_lossy: bool,
    pub original_line_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextReadOutcome {
    pub source_id: String,
    pub start_line: u64,
    pub end_line: u64,
    pub lines: Vec<ContextReadLine>,
    pub backtracked_bytes: u64,
    pub forward_bytes: u64,
    pub before_truncated: bool,
    pub content_truncated: bool,
}

pub fn read_referenced_context(
    root: &SafeRoot,
    reference: &MatchReferenceData,
    before_lines: usize,
    after_lines: usize,
    limits: ContextReadLimits,
) -> Result<ContextReadOutcome, ContextReadError> {
    limits.validate(before_lines, after_lines)?;
    let safe_file = open_referenced_file(root, reference)?;
    let file_size = safe_file.size();
    let mut file = safe_file.into_file();
    verify_reference_bytes(&mut file, file_size, reference)?;

    let mut backtrack = find_context_start(
        &mut file,
        reference.line_start_offset,
        before_lines,
        limits.max_backtrack_bytes,
        limits.read_buffer_bytes,
    )?;

    let required_match_bytes = reference
        .match_end_offset()
        .checked_sub(backtrack.start_offset)
        .ok_or(ContextReadError::FileChanged)?;
    if required_match_bytes > limits.max_forward_bytes {
        backtrack = BacktrackResult {
            start_offset: reference.line_start_offset,
            actual_before: 0,
            bytes_read: backtrack.bytes_read,
            truncated: before_lines > 0,
        };
    }
    let required_match_bytes = reference
        .match_end_offset()
        .checked_sub(backtrack.start_offset)
        .ok_or(ContextReadError::FileChanged)?;
    if required_match_bytes > limits.max_forward_bytes {
        return Err(ContextReadError::MatchOutsideContextBudget);
    }

    let start_line =
        reference
            .line_number
            .checked_sub(u64::try_from(backtrack.actual_before).map_err(|_| {
                ContextReadError::InvalidRequest("before_lines cannot be represented")
            })?)
            .ok_or(ContextReadError::FileChanged)?;
    let target_lines = backtrack
        .actual_before
        .saturating_add(1)
        .saturating_add(after_lines);
    let forward = read_forward_lines(
        &mut file,
        backtrack.start_offset,
        start_line,
        target_lines,
        limits,
    )?;
    let end_line = forward
        .lines
        .last()
        .map_or(start_line, |line| line.line_number);

    Ok(ContextReadOutcome {
        source_id: reference.source_id.clone(),
        start_line,
        end_line,
        lines: forward.lines,
        backtracked_bytes: backtrack.bytes_read,
        forward_bytes: forward.bytes_read,
        before_truncated: backtrack.truncated,
        content_truncated: forward.truncated,
    })
}

fn verify_reference_bytes(
    file: &mut std::fs::File,
    file_size: u64,
    reference: &MatchReferenceData,
) -> Result<(), ContextReadError> {
    reference
        .validate()
        .map_err(MatchReferenceFileError::from)?;
    if reference.match_end_offset() > file_size {
        return Err(ContextReadError::FileChanged);
    }

    if reference.line_start_offset > 0 {
        file.seek(SeekFrom::Start(reference.line_start_offset - 1))?;
        let mut separator = [0_u8; 1];
        file.read_exact(&mut separator)
            .map_err(map_reference_read_error)?;
        if separator[0] != b'\n' {
            return Err(ContextReadError::FileChanged);
        }
    }

    file.seek(SeekFrom::Start(reference.match_byte_offset))?;
    let mut observed = vec![0_u8; reference.keyword.len()];
    file.read_exact(&mut observed)
        .map_err(map_reference_read_error)?;
    let expected = reference.keyword.as_bytes();
    let matches = observed.iter().zip(expected).all(|(left, right)| {
        fold_ascii(*left, reference.case_sensitive) == fold_ascii(*right, reference.case_sensitive)
    });
    if !matches {
        return Err(ContextReadError::FileChanged);
    }

    Ok(())
}

fn map_reference_read_error(error: std::io::Error) -> ContextReadError {
    if error.kind() == ErrorKind::UnexpectedEof {
        ContextReadError::FileChanged
    } else {
        ContextReadError::Io(error)
    }
}

#[derive(Debug, Clone, Copy)]
struct BacktrackResult {
    start_offset: u64,
    actual_before: usize,
    bytes_read: u64,
    truncated: bool,
}

fn find_context_start(
    file: &mut std::fs::File,
    match_line_start: u64,
    before_lines: usize,
    max_backtrack_bytes: u64,
    read_buffer_bytes: usize,
) -> Result<BacktrackResult, ContextReadError> {
    if before_lines == 0 || match_line_start == 0 {
        return Ok(BacktrackResult {
            start_offset: match_line_start,
            actual_before: 0,
            bytes_read: 0,
            truncated: false,
        });
    }

    let lower_bound = match_line_start.saturating_sub(max_backtrack_bytes);
    let target_newlines = before_lines.saturating_add(1);
    let mut cursor = match_line_start;
    let mut buffer = vec![0_u8; read_buffer_bytes];
    let mut newline_offsets = Vec::with_capacity(target_newlines);
    let mut bytes_read = 0_u64;

    while cursor > lower_bound && newline_offsets.len() < target_newlines {
        let chunk_len_u64 = (cursor - lower_bound).min(read_buffer_bytes as u64);
        let chunk_len = usize::try_from(chunk_len_u64)
            .map_err(|_| ContextReadError::InvalidLimits("read buffer cannot be represented"))?;
        let chunk_start = cursor - chunk_len_u64;
        file.seek(SeekFrom::Start(chunk_start))?;
        file.read_exact(&mut buffer[..chunk_len])
            .map_err(map_reference_read_error)?;
        bytes_read = bytes_read.saturating_add(chunk_len_u64);

        for index in (0..chunk_len).rev() {
            if buffer[index] == b'\n' {
                newline_offsets.push(chunk_start + index as u64);
                if newline_offsets.len() == target_newlines {
                    break;
                }
            }
        }
        cursor = chunk_start;
    }

    if newline_offsets.len() >= target_newlines {
        return Ok(BacktrackResult {
            start_offset: newline_offsets[before_lines] + 1,
            actual_before: before_lines,
            bytes_read,
            truncated: false,
        });
    }

    if lower_bound == 0 {
        return Ok(BacktrackResult {
            start_offset: 0,
            actual_before: newline_offsets.len().min(before_lines),
            bytes_read,
            truncated: false,
        });
    }

    let start_offset = newline_offsets
        .last()
        .map_or(match_line_start, |offset| offset + 1);
    Ok(BacktrackResult {
        start_offset,
        actual_before: newline_offsets.len().saturating_sub(1).min(before_lines),
        bytes_read,
        truncated: true,
    })
}

#[derive(Debug)]
struct ForwardResult {
    lines: Vec<ContextReadLine>,
    bytes_read: u64,
    truncated: bool,
}

fn read_forward_lines(
    file: &mut std::fs::File,
    start_offset: u64,
    start_line: u64,
    target_lines: usize,
    limits: ContextReadLimits,
) -> Result<ForwardResult, ContextReadError> {
    file.seek(SeekFrom::Start(start_offset))?;
    let mut buffer = vec![0_u8; limits.read_buffer_bytes];
    let mut builder = ContextLineBuilder::new(limits.max_line_bytes);
    let mut lines = Vec::with_capacity(target_lines);
    let mut line_number = start_line;
    let mut bytes_read = 0_u64;
    let mut returned_content_bytes = 0_usize;

    loop {
        if lines.len() >= target_lines {
            return Ok(ForwardResult {
                lines,
                bytes_read,
                truncated: false,
            });
        }
        if bytes_read >= limits.max_forward_bytes {
            if builder.has_bytes() {
                let line = builder.finish(line_number, true);
                push_context_line(
                    &mut lines,
                    line,
                    &mut returned_content_bytes,
                    limits.max_returned_content_bytes,
                )?;
            }
            return Ok(ForwardResult {
                lines,
                bytes_read,
                truncated: true,
            });
        }

        let remaining = limits.max_forward_bytes - bytes_read;
        let read_size = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let count = match file.read(&mut buffer[..read_size]) {
            Ok(0) => {
                if builder.has_bytes() {
                    let line = builder.finish(line_number, false);
                    push_context_line(
                        &mut lines,
                        line,
                        &mut returned_content_bytes,
                        limits.max_returned_content_bytes,
                    )?;
                }
                return Ok(ForwardResult {
                    lines,
                    bytes_read,
                    truncated: false,
                });
            }
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(ContextReadError::Io(error)),
        };

        for byte in &buffer[..count] {
            bytes_read += 1;
            if *byte == b'\n' {
                let line = builder.finish(line_number, false);
                push_context_line(
                    &mut lines,
                    line,
                    &mut returned_content_bytes,
                    limits.max_returned_content_bytes,
                )?;
                if lines.len() >= target_lines {
                    return Ok(ForwardResult {
                        lines,
                        bytes_read,
                        truncated: false,
                    });
                }
                line_number = line_number.saturating_add(1);
            } else {
                builder.push(*byte);
            }
        }
    }
}

fn push_context_line(
    lines: &mut Vec<ContextReadLine>,
    mut line: ContextReadLine,
    returned_content_bytes: &mut usize,
    max_returned_content_bytes: usize,
) -> Result<(), ContextReadError> {
    let remaining = max_returned_content_bytes.saturating_sub(*returned_content_bytes);
    if remaining == 0 {
        return Err(ContextReadError::ReturnedContentLimit);
    }
    if line.content.len() > remaining {
        truncate_utf8(&mut line.content, remaining);
        line.content_truncated = true;
    }
    *returned_content_bytes = returned_content_bytes.saturating_add(line.content.len());
    lines.push(line);
    Ok(())
}

fn truncate_utf8(content: &mut String, max_bytes: usize) {
    if content.len() <= max_bytes {
        return;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    content.truncate(boundary);
}

#[derive(Debug)]
struct ContextLineBuilder {
    preview: Vec<u8>,
    original_bytes: u64,
    preview_limit: usize,
}

impl ContextLineBuilder {
    fn new(preview_limit: usize) -> Self {
        Self {
            preview: Vec::with_capacity(preview_limit),
            original_bytes: 0,
            preview_limit,
        }
    }

    fn has_bytes(&self) -> bool {
        self.original_bytes > 0
    }

    fn push(&mut self, byte: u8) {
        self.original_bytes += 1;
        if self.preview.len() < self.preview_limit {
            self.preview.push(byte);
        }
    }

    fn finish(&mut self, line_number: u64, forced_truncation: bool) -> ContextReadLine {
        let complete_preview = self.preview.len() as u64 == self.original_bytes;
        if complete_preview && self.preview.last() == Some(&b'\r') {
            self.preview.pop();
        }
        let bytes = std::mem::take(&mut self.preview);
        let (content, content_lossy) = match String::from_utf8(bytes) {
            Ok(content) => (content, false),
            Err(error) => (String::from_utf8_lossy(error.as_bytes()).into_owned(), true),
        };
        let line = ContextReadLine {
            line_number,
            content,
            content_truncated: forced_truncation || !complete_preview,
            content_lossy,
            original_line_bytes: self.original_bytes,
        };
        self.preview = Vec::with_capacity(self.preview_limit);
        self.original_bytes = 0;
        line
    }
}

fn fold_ascii(byte: u8, case_sensitive: bool) -> u8 {
    if case_sensitive {
        byte
    } else {
        byte.to_ascii_lowercase()
    }
}

#[derive(Debug, Error)]
pub enum ContextReadError {
    #[error("invalid context request: {0}")]
    InvalidRequest(&'static str),

    #[error("invalid context limits: {0}")]
    InvalidLimits(&'static str),

    #[error("referenced log file cannot be used")]
    Reference(#[from] MatchReferenceFileError),

    #[error("referenced log file changed after the match was created")]
    FileChanged,

    #[error("the match is outside the bounded context read budget")]
    MatchOutsideContextBudget,

    #[error("returned context content reached the service limit")]
    ReturnedContentLimit,

    #[error("context read failed")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use crate::{MatchReferenceStore, ScanRequest, scan_reader};

    use super::*;

    fn create_reference(root: &SafeRoot, relative_path: &str, keyword: &str) -> MatchReferenceData {
        let safe_file = root
            .open_regular_file(relative_path)
            .expect("fixture should open safely");
        let identity = safe_file.identity();
        let size = safe_file.size();
        let mut file = safe_file.into_file();
        let outcome = scan_reader(&mut file, &ScanRequest::new(keyword))
            .expect("fixture scan should succeed");
        let scan_match = outcome.results.first().expect("fixture should match");
        MatchReferenceData::from_scan_match(
            "payment-test",
            relative_path,
            identity,
            size,
            keyword,
            false,
            scan_match,
        )
        .expect("reference data should be valid")
    }

    #[test]
    fn scans_stores_resolves_and_reads_bounded_context() {
        let directory = tempdir().expect("temporary directory should be created");
        let content = concat!(
            "first line\n",
            "before line\n",
            "ERROR traceId=abc123 payment failed\n",
            "    at payment::authorize\n",
            "Caused by: forbidden\n",
            "last line\n",
        );
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let reference = create_reference(&root, "application.log", "abc123");
        let store = MatchReferenceStore::new(10, std::time::Duration::from_secs(60))
            .expect("store should be created");
        let token = store.insert(reference).expect("reference should be stored");
        let resolved = store.resolve(&token).expect("reference should resolve");

        let context = read_referenced_context(&root, &resolved, 1, 2, ContextReadLimits::default())
            .expect("context should be read");

        assert_eq!(context.start_line, 2);
        assert_eq!(context.end_line, 5);
        assert_eq!(context.lines.len(), 4);
        assert_eq!(context.lines[0].content, "before line");
        assert!(context.lines[1].content.contains("abc123"));
        assert_eq!(context.lines[2].content, "    at payment::authorize");
        assert_eq!(context.lines[3].content, "Caused by: forbidden");
        assert!(!context.before_truncated);
        assert!(!context.content_truncated);
    }

    #[test]
    fn detects_same_inode_content_rewrite_by_rechecking_keyword() {
        let directory = tempdir().expect("temporary directory should be created");
        let path = directory.path().join("application.log");
        fs::write(&path, "prefix abc123 suffix\n").expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let reference = create_reference(&root, "application.log", "abc123");

        fs::write(&path, "prefix changed suffix\n").expect("fixture should be rewritten");

        assert!(matches!(
            read_referenced_context(&root, &reference, 0, 0, ContextReadLimits::default()),
            Err(ContextReadError::FileChanged)
                | Err(ContextReadError::Reference(
                    MatchReferenceFileError::FileChanged
                ))
        ));
    }

    #[test]
    fn bounds_backtracking_when_previous_line_is_too_large() {
        let directory = tempdir().expect("temporary directory should be created");
        let mut content = vec![b'x'; 1024];
        content.extend_from_slice(b"\nERROR abc123\nafter\n");
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let reference = create_reference(&root, "application.log", "abc123");
        let limits = ContextReadLimits {
            max_backtrack_bytes: 32,
            read_buffer_bytes: 8,
            ..ContextReadLimits::default()
        };

        let context = read_referenced_context(&root, &reference, 1, 1, limits)
            .expect("bounded context should be read");

        assert!(context.before_truncated);
        assert_eq!(context.start_line, 2);
        assert_eq!(context.lines[0].content, "ERROR abc123");
        assert_eq!(context.lines[1].content, "after");
    }

    #[test]
    fn truncates_very_long_context_lines_without_unbounded_allocation() {
        let directory = tempdir().expect("temporary directory should be created");
        let mut content = b"ERROR abc123\n".to_vec();
        content.extend(std::iter::repeat_n(b'y', 1024));
        content.push(b'\n');
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let reference = create_reference(&root, "application.log", "abc123");
        let limits = ContextReadLimits {
            max_line_bytes: 32,
            max_returned_content_bytes: 128,
            read_buffer_bytes: 16,
            ..ContextReadLimits::default()
        };

        let context = read_referenced_context(&root, &reference, 0, 1, limits)
            .expect("context should be read");

        assert_eq!(context.lines.len(), 2);
        assert_eq!(context.lines[1].content.len(), 32);
        assert!(context.lines[1].content_truncated);
        assert_eq!(context.lines[1].original_line_bytes, 1024);
    }

    #[test]
    fn validates_context_limits() {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(directory.path().join("application.log"), "ERROR abc123\n")
            .expect("fixture should be written");
        let root = SafeRoot::open(directory.path()).expect("root should open");
        let reference = create_reference(&root, "application.log", "abc123");
        let limits = ContextReadLimits {
            max_returned_content_bytes: 8,
            max_line_bytes: 16,
            ..ContextReadLimits::default()
        };

        assert!(matches!(
            read_referenced_context(&root, &reference, 0, 0, limits),
            Err(ContextReadError::InvalidLimits(_))
        ));
    }
}
