use std::{io::{Read, Seek, SeekFrom}, sync::Arc};

use thiserror::Error;

use crate::{MatchReferenceData, SourceRegistry, SourceRegistryError};

pub const MAX_CONTEXT_LINES_PER_SIDE: usize = 50;
pub const MAX_CONTEXT_LINE_BYTES: usize = 1024 * 1024;
pub const MAX_CONTEXT_CONTENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CONTEXT_SCAN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextLimits {
    pub max_lines_per_side: usize,
    pub max_line_bytes: usize,
    pub max_content_bytes: usize,
    pub max_before_scan_bytes: u64,
    pub max_forward_scan_bytes: u64,
    pub read_buffer_bytes: usize,
}

impl Default for ContextLimits {
    fn default() -> Self {
        Self {
            max_lines_per_side: 50,
            max_line_bytes: 16 * 1024,
            max_content_bytes: 512 * 1024,
            max_before_scan_bytes: 4 * 1024 * 1024,
            max_forward_scan_bytes: 4 * 1024 * 1024,
            read_buffer_bytes: 64 * 1024,
        }
    }
}

impl ContextLimits {
    fn validate(self, before_lines: usize, after_lines: usize) -> Result<(), ContextReadError> {
        if self.max_lines_per_side == 0
            || self.max_lines_per_side > MAX_CONTEXT_LINES_PER_SIDE
            || before_lines > self.max_lines_per_side
            || after_lines > self.max_lines_per_side
        {
            return Err(ContextReadError::InvalidLimits(
                "context line count is outside the service limit",
            ));
        }
        if self.max_line_bytes == 0 || self.max_line_bytes > MAX_CONTEXT_LINE_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "context line byte limit is outside the service limit",
            ));
        }
        if self.max_content_bytes < self.max_line_bytes
            || self.max_content_bytes > MAX_CONTEXT_CONTENT_BYTES
        {
            return Err(ContextReadError::InvalidLimits(
                "context content limit must cover one line and remain within the service limit",
            ));
        }
        if self.max_before_scan_bytes == 0
            || self.max_before_scan_bytes > MAX_CONTEXT_SCAN_BYTES
            || self.max_forward_scan_bytes == 0
            || self.max_forward_scan_bytes > MAX_CONTEXT_SCAN_BYTES
            || self.read_buffer_bytes == 0
            || self.read_buffer_bytes > 1024 * 1024
        {
            return Err(ContextReadError::InvalidLimits(
                "context scan or buffer limit is outside the service limit",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextLine {
    pub line_number: u64,
    pub content: String,
    pub content_truncated: bool,
    pub content_lossy: bool,
    pub original_line_bytes: u64,
    pub is_match_line: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextOutcome {
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub start_line: u64,
    pub end_line: u64,
    pub lines: Vec<ContextLine>,
    pub before_truncated: bool,
    pub after_truncated: bool,
    pub returned_content_bytes: usize,
}

#[derive(Debug)]
pub struct ContextReader {
    registry: Arc<SourceRegistry>,
    limits: ContextLimits,
}

impl ContextReader {
    pub fn new(registry: Arc<SourceRegistry>, limits: ContextLimits) -> Result<Self, ContextReadError> {
        limits.validate(0, 0)?;
        Ok(Self { registry, limits })
    }

    pub fn read(
        &self,
        reference: &MatchReferenceData,
        before_lines: usize,
        after_lines: usize,
    ) -> Result<ContextOutcome, ContextReadError> {
        self.limits.validate(before_lines, after_lines)?;
        reference.validate().map_err(ContextReadError::InvalidReference)?;
        let snapshot = reference.snapshot();
        let source = self
            .registry
            .get(snapshot.source_id())
            .ok_or(ContextReadError::UnknownSource)?;
        let safe_file = source.open_snapshot_file(snapshot)?;
        let mut file = safe_file.into_file();
        verify_reference(&mut file, reference)?;

        let (before_candidates, before_scan_truncated) = read_before_lines(
            &mut file,
            reference.line_start_offset(),
            reference.line_number(),
            before_lines,
            self.limits,
        )?;
        let (match_line, after_candidates, after_scan_truncated) = read_match_and_after(
            &mut file,
            reference,
            after_lines,
            self.limits,
        )?;

        let mut returned_content_bytes = match_line.content.len();
        let mut selected_before = Vec::new();
        let mut before_budget_truncated = false;
        for line in before_candidates.into_iter().rev() {
            if returned_content_bytes.saturating_add(line.content.len()) > self.limits.max_content_bytes {
                before_budget_truncated = true;
                break;
            }
            returned_content_bytes += line.content.len();
            selected_before.push(line);
        }
        selected_before.reverse();

        let mut selected_after = Vec::new();
        let mut after_budget_truncated = false;
        for line in after_candidates {
            if returned_content_bytes.saturating_add(line.content.len()) > self.limits.max_content_bytes {
                after_budget_truncated = true;
                break;
            }
            returned_content_bytes += line.content.len();
            selected_after.push(line);
        }

        let mut lines = Vec::with_capacity(selected_before.len() + 1 + selected_after.len());
        lines.extend(selected_before);
        lines.push(match_line);
        lines.extend(selected_after);
        let start_line = lines.first().map_or(reference.line_number(), |line| line.line_number);
        let end_line = lines.last().map_or(reference.line_number(), |line| line.line_number);

        Ok(ContextOutcome {
            source_id: snapshot.source_id().to_owned(),
            file_id: snapshot.file_id().to_owned(),
            file_name: snapshot.display_name(),
            start_line,
            end_line,
            lines,
            before_truncated: before_scan_truncated || before_budget_truncated,
            after_truncated: after_scan_truncated || after_budget_truncated,
            returned_content_bytes,
        })
    }
}

fn verify_reference(file: &mut std::fs::File, reference: &MatchReferenceData) -> Result<(), ContextReadError> {
    if reference.line_start_offset() > 0 {
        file.seek(SeekFrom::Start(reference.line_start_offset() - 1))?;
        let mut separator = [0_u8; 1];
        file.read_exact(&mut separator).map_err(map_stale_read)?;
        if separator[0] != b'\n' {
            return Err(ContextReadError::FileChanged);
        }
    }

    file.seek(SeekFrom::Start(reference.match_byte_offset()))?;
    let mut observed = vec![0_u8; reference.keyword().len()];
    file.read_exact(&mut observed).map_err(map_stale_read)?;
    let matches = observed.iter().zip(reference.keyword().as_bytes()).all(|(left, right)| {
        fold_ascii(*left, reference.case_sensitive()) == fold_ascii(*right, reference.case_sensitive())
    });
    if !matches {
        return Err(ContextReadError::FileChanged);
    }
    Ok(())
}

fn map_stale_read(error: std::io::Error) -> ContextReadError {
    if error.kind() == std::io::ErrorKind::UnexpectedEof {
        ContextReadError::FileChanged
    } else {
        ContextReadError::Io(error)
    }
}

fn read_before_lines(
    file: &mut std::fs::File,
    line_start: u64,
    line_number: u64,
    requested: usize,
    limits: ContextLimits,
) -> Result<(Vec<ContextLine>, bool), ContextReadError> {
    if requested == 0 || line_start == 0 {
        return Ok((Vec::new(), false));
    }
    let scan_bytes = line_start.min(limits.max_before_scan_bytes);
    let window_start = line_start - scan_bytes;
    file.seek(SeekFrom::Start(window_start))?;
    let buffer_len = usize::try_from(scan_bytes).map_err(|_| ContextReadError::PositionOverflow)?;
    let mut bytes = vec![0_u8; buffer_len];
    file.read_exact(&mut bytes).map_err(map_stale_read)?;
    if bytes.last() != Some(&b'\n') {
        return Err(ContextReadError::FileChanged);
    }

    let usable_start = if window_start == 0 {
        0
    } else {
        bytes.iter().position(|byte| *byte == b'\n').map_or(bytes.len(), |index| index + 1)
    };
    let usable = &bytes[usable_start..bytes.len() - 1];
    let segments: Vec<&[u8]> = if usable.is_empty() {
        Vec::new()
    } else {
        usable.split(|byte| *byte == b'\n').collect()
    };
    let take = requested.min(segments.len());
    let first = segments.len().saturating_sub(take);
    let first_line = line_number
        .checked_sub(u64::try_from(take).map_err(|_| ContextReadError::PositionOverflow)?)
        .ok_or(ContextReadError::PositionOverflow)?;
    let mut result = Vec::with_capacity(take);
    for (index, segment) in segments[first..].iter().enumerate() {
        result.push(context_line_from_bytes(
            first_line + u64::try_from(index).map_err(|_| ContextReadError::PositionOverflow)?,
            segment,
            limits.max_line_bytes,
            false,
        ));
    }
    Ok((result, take < requested && window_start > 0))
}

fn read_match_and_after(
    file: &mut std::fs::File,
    reference: &MatchReferenceData,
    after_lines: usize,
    limits: ContextLimits,
) -> Result<(ContextLine, Vec<ContextLine>, bool), ContextReadError> {
    file.seek(SeekFrom::Start(reference.line_start_offset()))?;
    let mut remaining = limits.max_forward_scan_bytes;
    let match_relative = reference
        .match_byte_offset()
        .checked_sub(reference.line_start_offset())
        .ok_or(ContextReadError::FileChanged)?;
    let match_line = read_streamed_line(
        file,
        reference.line_number(),
        Some(match_relative),
        reference.keyword().len(),
        limits.max_line_bytes,
        limits.read_buffer_bytes,
        &mut remaining,
        true,
    )?
    .ok_or(ContextReadError::FileChanged)?;
    if !match_line.complete {
        return Err(ContextReadError::ScanLimitBeforeMatchLineEnd);
    }

    let mut after = Vec::with_capacity(after_lines);
    let mut truncated = false;
    for index in 0..after_lines {
        match read_streamed_line(
            file,
            reference.line_number() + u64::try_from(index + 1).map_err(|_| ContextReadError::PositionOverflow)?,
            None,
            0,
            limits.max_line_bytes,
            limits.read_buffer_bytes,
            &mut remaining,
            false,
        )? {
            Some(line) if line.complete => after.push(line.line),
            Some(_) => { truncated = true; break; }
            None => break,
        }
    }
    Ok((match_line.line, after, truncated))
}

struct StreamedLine {
    line: ContextLine,
    complete: bool,
}

#[allow(clippy::too_many_arguments)]
fn read_streamed_line(
    file: &mut std::fs::File,
    line_number: u64,
    match_relative: Option<u64>,
    keyword_len: usize,
    max_line_bytes: usize,
    read_buffer_bytes: usize,
    remaining: &mut u64,
    is_match_line: bool,
) -> Result<Option<StreamedLine>, ContextReadError> {
    if *remaining == 0 {
        return Ok(None);
    }
    let window_start = match match_relative {
        Some(offset) => offset.saturating_sub(u64::try_from(max_line_bytes.saturating_sub(keyword_len) / 2).unwrap_or(0)),
        None => 0,
    };
    let window_end = window_start.saturating_add(u64::try_from(max_line_bytes).map_err(|_| ContextReadError::PositionOverflow)?);
    let mut preview = Vec::with_capacity(max_line_bytes);
    let mut line_bytes = 0_u64;
    let mut complete = false;
    let mut buffer = vec![0_u8; read_buffer_bytes];

    while *remaining > 0 {
        let read_size = buffer.len().min(usize::try_from(*remaining).unwrap_or(usize::MAX));
        let count = file.read(&mut buffer[..read_size])?;
        if count == 0 {
            complete = true;
            break;
        }
        for byte in &buffer[..count] {
            *remaining -= 1;
            if *byte == b'\n' {
                complete = true;
                break;
            }
            if line_bytes >= window_start && line_bytes < window_end {
                preview.push(*byte);
            }
            line_bytes = line_bytes.checked_add(1).ok_or(ContextReadError::PositionOverflow)?;
        }
        if complete {
            break;
        }
    }

    if line_bytes == 0 && preview.is_empty() && complete {
        return Ok(None);
    }
    if complete && window_start == 0 && preview.last() == Some(&b'\r') {
        preview.pop();
    }
    let content_truncated = window_start > 0 || u64::try_from(preview.len()).unwrap_or(u64::MAX) < line_bytes;
    let (content, content_lossy) = bytes_to_string(preview);
    Ok(Some(StreamedLine {
        line: ContextLine {
            line_number,
            content,
            content_truncated,
            content_lossy,
            original_line_bytes: line_bytes,
            is_match_line,
        },
        complete,
    }))
}

fn context_line_from_bytes(
    line_number: u64,
    bytes: &[u8],
    max_line_bytes: usize,
    is_match_line: bool,
) -> ContextLine {
    let mut content = bytes;
    if content.last() == Some(&b'\r') {
        content = &content[..content.len() - 1];
    }
    let preview_len = content.len().min(max_line_bytes);
    let (content, content_lossy) = bytes_to_string(content[..preview_len].to_vec());
    ContextLine {
        line_number,
        content,
        content_truncated: preview_len < bytes.len(),
        content_lossy,
        original_line_bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
        is_match_line,
    }
}

fn bytes_to_string(bytes: Vec<u8>) -> (String, bool) {
    match String::from_utf8(bytes) {
        Ok(content) => (content, false),
        Err(error) => (String::from_utf8_lossy(error.as_bytes()).into_owned(), true),
    }
}

fn fold_ascii(byte: u8, case_sensitive: bool) -> u8 {
    if case_sensitive { byte } else { byte.to_ascii_lowercase() }
}

#[derive(Debug, Error)]
pub enum ContextReadError {
    #[error("invalid context limits: {0}")]
    InvalidLimits(&'static str),

    #[error("match reference data is invalid")]
    InvalidReference(#[source] crate::MatchReferenceError),

    #[error("unknown log source for match reference")]
    UnknownSource,

    #[error("referenced log file changed after search")]
    FileChanged,

    #[error("context scan limit was reached before the match line ended")]
    ScanLimitBeforeMatchLineEnd,

    #[error("context byte or line position overflowed")]
    PositionOverflow,

    #[error(transparent)]
    SourceRegistry(#[from] SourceRegistryError),

    #[error("context file read failed")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::{AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, QueryMatch, SourceFileSnapshot};
    use tempfile::tempdir;

    use super::*;

    fn registry(root: &std::path::Path) -> Arc<SourceRegistry> {
        Arc::new(SourceRegistry::from_config(AppConfig {
            version: CONFIG_VERSION,
            sources: vec![LogSourceConfig {
                source_id: "payment-test".to_owned(),
                name: "Payment".to_owned(),
                description: String::new(),
                service: "payment-service".to_owned(),
                environment: "test".to_owned(),
                tags: Vec::new(),
                enabled: true,
                encoding: Encoding::Utf8,
                root: root.to_path_buf(),
                files: vec![PathBuf::from("application.log")],
                directories: Vec::new(),
                timestamp_rule: None,
            }],
            limits: LimitsConfig::default(),
        }).expect("registry should build"))
    }

    fn reference(registry: &SourceRegistry, content: &str) -> MatchReferenceData {
        let source = registry.get("payment-test").expect("source");
        let snapshot = source.snapshot_files(10).expect("snapshot").remove(0);
        let match_offset = u64::try_from(content.find("abc123").expect("keyword")).expect("offset");
        let line_start = u64::try_from(content[..usize::try_from(match_offset).expect("offset")].rfind('\n').map_or(0, |index| index + 1)).expect("line start");
        let line_number = u64::try_from(content[..usize::try_from(line_start).expect("line start")].bytes().filter(|byte| *byte == b'\n').count() + 1).expect("line number");
        let query_match = QueryMatch {
            source_id: "payment-test".to_owned(),
            file_id: snapshot.file_id().to_owned(),
            file_name: snapshot.display_name(),
            line_number,
            timestamp: None,
            content: "abc123".to_owned(),
            content_truncated: false,
            content_lossy: false,
            original_line_bytes: 20,
            line_start_offset: line_start,
            match_byte_offset: match_offset,
        };
        MatchReferenceData::from_query_match(snapshot, &query_match, "abc123", false)
            .expect("reference")
    }

    #[test]
    fn reads_bounded_before_match_and_after_context() {
        let root = tempdir().expect("root");
        let content = "one\ntwo\nERROR traceId=abc123 failed\nstack one\nstack two\nlast\n";
        fs::write(root.path().join("application.log"), content).expect("write");
        let registry = registry(root.path());
        let reference = reference(&registry, content);
        let reader = ContextReader::new(Arc::clone(&registry), ContextLimits::default()).expect("reader");

        let outcome = reader.read(&reference, 2, 2).expect("context");
        assert_eq!(outcome.start_line, 1);
        assert_eq!(outcome.end_line, 5);
        assert_eq!(outcome.lines.len(), 5);
        assert!(outcome.lines[2].is_match_line);
        assert!(outcome.lines[2].content.contains("abc123"));
    }

    #[test]
    fn detects_same_inode_content_rewrite_at_match_position() {
        let root = tempdir().expect("root");
        let path = root.path().join("application.log");
        let content = "prefix abc123 suffix\n";
        fs::write(&path, content).expect("write");
        let registry = registry(root.path());
        let reference = reference(&registry, content);
        fs::write(&path, "prefix changed suffix\n").expect("rewrite");
        let reader = ContextReader::new(registry, ContextLimits::default()).expect("reader");

        assert!(reader.read(&reference, 0, 0).is_err());
    }

    #[test]
    fn long_match_line_preview_contains_keyword() {
        let root = tempdir().expect("root");
        let mut content = "x".repeat(200);
        content.push_str("abc123");
        content.push_str(&"y".repeat(200));
        content.push('\n');
        fs::write(root.path().join("application.log"), &content).expect("write");
        let registry = registry(root.path());
        let reference = reference(&registry, &content);
        let limits = ContextLimits { max_line_bytes: 32, max_content_bytes: 128, ..ContextLimits::default() };
        let reader = ContextReader::new(registry, limits).expect("reader");

        let outcome = reader.read(&reference, 0, 0).expect("context");
        assert!(outcome.lines[0].content.contains("abc123"));
        assert!(outcome.lines[0].content_truncated);
    }
}
