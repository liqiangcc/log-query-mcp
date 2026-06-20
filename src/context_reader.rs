use std::{
    borrow::Cow,
    collections::VecDeque,
    io::{BufRead, BufReader, ErrorKind, Read, Seek, SeekFrom},
    time::Instant,
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ConfiguredSource, LimitsConfig, MatchReferenceData, QueryStateError, SourceRegistryError,
};

const INTERRUPT_CHECK_INTERVAL_BYTES: u64 = 4 * 1024;
pub const DEFAULT_CONTEXT_SCAN_BYTES: u64 = 4 * 1024 * 1024;
pub const MAX_CONTEXT_SCAN_BYTES: u64 = 64 * 1024 * 1024;
pub const MAX_CONTEXT_READ_BUFFER_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextReadLimits {
    pub max_lines_per_side: usize,
    pub max_line_bytes: usize,
    pub max_returned_content_bytes: usize,
    pub max_before_scan_bytes: u64,
    pub max_forward_scan_bytes: u64,
    pub read_buffer_bytes: usize,
}

impl ContextReadLimits {
    #[must_use]
    pub fn from_service_limits(limits: &LimitsConfig) -> Self {
        let scan_bytes = limits
            .max_scan_bytes_per_page
            .min(DEFAULT_CONTEXT_SCAN_BYTES)
            .max(1);
        Self {
            max_lines_per_side: limits.max_context_lines_per_side,
            max_line_bytes: limits.max_line_bytes,
            max_returned_content_bytes: limits.max_returned_content_bytes,
            max_before_scan_bytes: scan_bytes,
            max_forward_scan_bytes: scan_bytes,
            read_buffer_bytes: 64 * 1024,
        }
    }

    fn validate(self, before_lines: usize, after_lines: usize) -> Result<(), ContextReadError> {
        if self.max_lines_per_side == 0
            || before_lines > self.max_lines_per_side
            || after_lines > self.max_lines_per_side
        {
            return Err(ContextReadError::InvalidRequest(
                "context line count exceeds the configured limit",
            ));
        }
        if self.max_line_bytes == 0 {
            return Err(ContextReadError::InvalidLimits(
                "max_line_bytes must be greater than zero",
            ));
        }
        if self.max_returned_content_bytes < self.max_line_bytes {
            return Err(ContextReadError::InvalidLimits(
                "returned content budget must retain one full preview",
            ));
        }
        if self.max_before_scan_bytes == 0
            || self.max_before_scan_bytes > MAX_CONTEXT_SCAN_BYTES
            || self.max_forward_scan_bytes == 0
            || self.max_forward_scan_bytes > MAX_CONTEXT_SCAN_BYTES
        {
            return Err(ContextReadError::InvalidLimits(
                "context scan byte limit is outside the service boundary",
            ));
        }
        if self.read_buffer_bytes == 0 || self.read_buffer_bytes > MAX_CONTEXT_READ_BUFFER_BYTES {
            return Err(ContextReadError::InvalidLimits(
                "context read buffer is outside the service boundary",
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
pub struct ContextReadOutcome {
    pub start_line: u64,
    pub end_line: u64,
    pub lines: Vec<ContextLine>,
    pub before_truncated: bool,
    pub after_truncated: bool,
    pub returned_content_bytes: usize,
    pub bytes_scanned: u64,
    pub truncated: bool,
}

pub fn read_referenced_context(
    source: &ConfiguredSource,
    reference: &MatchReferenceData,
    before_lines: usize,
    after_lines: usize,
    limits: ContextReadLimits,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<ContextReadOutcome, ContextReadError> {
    limits.validate(before_lines, after_lines)?;
    reference.validate()?;
    check_interrupted(cancellation, deadline)?;

    if source.descriptor().source_id != reference.source_id {
        return Err(ContextReadError::FileChanged);
    }
    let safe_file = source.open_configured_file(&reference.relative_path)?;
    if safe_file.identity() != reference.file_identity
        || safe_file.size() < reference.file_size_at_match
        || safe_file.size() < reference.match_end_offset()
    {
        return Err(ContextReadError::FileChanged);
    }

    let current_size = safe_file.size();
    let mut file = safe_file.into_file();
    read_context(
        &mut file,
        current_size,
        reference,
        before_lines,
        after_lines,
        limits,
        cancellation,
        deadline,
    )
}

pub fn read_context<R: Read + Seek>(
    reader: &mut R,
    file_size: u64,
    reference: &MatchReferenceData,
    before_lines: usize,
    after_lines: usize,
    limits: ContextReadLimits,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<ContextReadOutcome, ContextReadError> {
    limits.validate(before_lines, after_lines)?;
    reference.validate()?;
    if reference.file_size_at_match > file_size || reference.match_end_offset() > file_size {
        return Err(ContextReadError::FileChanged);
    }

    let before = read_before_lines(
        reader,
        reference,
        before_lines,
        limits,
        cancellation,
        deadline,
    )?;
    let forward = read_match_and_after(
        reader,
        reference,
        after_lines,
        limits,
        cancellation,
        deadline,
    )?;

    let mut returned_content_bytes = forward.match_line.content.len();
    if returned_content_bytes > limits.max_returned_content_bytes {
        return Err(ContextReadError::InvalidLimits(
            "match preview exceeds the returned content budget",
        ));
    }

    let mut selected_before = VecDeque::new();
    let mut before_budget_truncated = false;
    for line in before.lines.into_iter().rev() {
        if returned_content_bytes.saturating_add(line.content.len())
            > limits.max_returned_content_bytes
        {
            before_budget_truncated = true;
            break;
        }
        returned_content_bytes += line.content.len();
        selected_before.push_front(line);
    }

    let mut selected_after = Vec::new();
    let mut after_budget_truncated = false;
    for line in forward.after_lines {
        if returned_content_bytes.saturating_add(line.content.len())
            > limits.max_returned_content_bytes
        {
            after_budget_truncated = true;
            break;
        }
        returned_content_bytes += line.content.len();
        selected_after.push(line);
    }

    let mut lines = Vec::with_capacity(selected_before.len() + 1 + selected_after.len());
    lines.extend(selected_before);
    lines.push(forward.match_line);
    lines.extend(selected_after);

    let start_line = lines
        .first()
        .map(|line| line.line_number)
        .ok_or(ContextReadError::FileChanged)?;
    let end_line = lines
        .last()
        .map(|line| line.line_number)
        .ok_or(ContextReadError::FileChanged)?;
    let before_truncated = before.truncated || before_budget_truncated;
    let after_truncated = forward.after_truncated || after_budget_truncated;
    let line_truncated = lines.iter().any(|line| line.content_truncated);
    let bytes_scanned = before
        .bytes_scanned
        .checked_add(forward.bytes_scanned)
        .ok_or(ContextReadError::CounterOverflow)?;

    Ok(ContextReadOutcome {
        start_line,
        end_line,
        lines,
        before_truncated,
        after_truncated,
        returned_content_bytes,
        bytes_scanned,
        truncated: before_truncated || after_truncated || line_truncated,
    })
}

#[derive(Debug)]
struct BeforeRead {
    lines: Vec<ContextLine>,
    bytes_scanned: u64,
    truncated: bool,
}

fn read_before_lines<R: Read + Seek>(
    reader: &mut R,
    reference: &MatchReferenceData,
    requested_lines: usize,
    limits: ContextReadLimits,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<BeforeRead, ContextReadError> {
    if requested_lines == 0 || reference.line_start_offset == 0 {
        return Ok(BeforeRead {
            lines: Vec::new(),
            bytes_scanned: 0,
            truncated: false,
        });
    }

    let scan_bytes = reference
        .line_start_offset
        .min(limits.max_before_scan_bytes);
    let window_start = reference.line_start_offset - scan_bytes;
    let mut bytes_scanned = 0_u64;
    let aligned = if window_start == 0 {
        true
    } else {
        reader.seek(SeekFrom::Start(window_start - 1))?;
        let mut previous = [0_u8; 1];
        read_exact_controlled(
            reader,
            &mut previous,
            cancellation,
            deadline,
            &mut bytes_scanned,
        )?;
        previous[0] == b'\n'
    };

    reader.seek(SeekFrom::Start(window_start))?;
    let buffer_len = usize::try_from(scan_bytes)
        .map_err(|_| ContextReadError::InvalidLimits("before scan window is too large"))?;
    let mut bytes = vec![0_u8; buffer_len];
    read_exact_controlled(
        reader,
        &mut bytes,
        cancellation,
        deadline,
        &mut bytes_scanned,
    )?;
    if bytes.last() != Some(&b'\n') {
        return Err(ContextReadError::FileChanged);
    }

    let full_start = if aligned {
        0
    } else {
        bytes
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |index| index + 1)
    };
    let mut ranges = VecDeque::with_capacity(requested_lines);
    let mut current_start = full_start;
    let mut complete_lines = 0_usize;
    for (index, byte) in bytes.iter().enumerate().skip(full_start) {
        if *byte == b'\n' {
            complete_lines = complete_lines.saturating_add(1);
            if ranges.len() == requested_lines {
                ranges.pop_front();
            }
            if requested_lines > 0 {
                ranges.push_back((current_start, index));
            }
            current_start = index + 1;
        }
    }
    if current_start != bytes.len() {
        return Err(ContextReadError::FileChanged);
    }

    let actual_lines = ranges.len();
    let first_line_number = reference
        .line_number
        .checked_sub(u64::try_from(actual_lines).map_err(|_| ContextReadError::CounterOverflow)?)
        .ok_or(ContextReadError::FileChanged)?;
    let mut lines = Vec::with_capacity(actual_lines);
    for (index, (start, end)) in ranges.into_iter().enumerate() {
        lines.push(context_line_from_bytes(
            first_line_number
                .checked_add(u64::try_from(index).map_err(|_| ContextReadError::CounterOverflow)?)
                .ok_or(ContextReadError::CounterOverflow)?,
            &bytes[start..end],
            limits.max_line_bytes,
            false,
            None,
        ));
    }

    Ok(BeforeRead {
        lines,
        bytes_scanned,
        truncated: actual_lines < requested_lines
            && window_start > 0
            && complete_lines >= actual_lines,
    })
}

#[derive(Debug)]
struct ForwardRead {
    match_line: ContextLine,
    after_lines: Vec<ContextLine>,
    bytes_scanned: u64,
    after_truncated: bool,
}

fn read_match_and_after<R: Read + Seek>(
    reader: &mut R,
    reference: &MatchReferenceData,
    requested_after_lines: usize,
    limits: ContextReadLimits,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<ForwardRead, ContextReadError> {
    reader.seek(SeekFrom::Start(reference.line_start_offset))?;
    let mut buffered = BufReader::with_capacity(limits.read_buffer_bytes, reader);
    let mut remaining_scan_bytes = limits.max_forward_scan_bytes;
    let mut bytes_scanned = 0_u64;
    let relative_match = reference
        .match_byte_offset
        .checked_sub(reference.line_start_offset)
        .ok_or(ContextReadError::FileChanged)?;
    let match_spec = MatchSpec {
        relative_offset: relative_match,
        keyword: reference.keyword.as_bytes(),
        case_sensitive: reference.case_sensitive,
    };

    let raw_match = match read_line(
        &mut buffered,
        limits.max_line_bytes,
        &mut remaining_scan_bytes,
        &mut bytes_scanned,
        Some(match_spec),
        cancellation,
        deadline,
    )? {
        LineRead::Complete(line) => line,
        LineRead::Eof => return Err(ContextReadError::FileChanged),
        LineRead::BudgetExhausted => return Err(ContextReadError::MatchLineScanLimit),
    };
    let match_line = raw_match.into_context_line(
        reference.line_number,
        true,
        limits.max_line_bytes,
        Some((reference.keyword.as_bytes(), reference.case_sensitive)),
    );

    let mut after_lines = Vec::with_capacity(requested_after_lines);
    let mut after_truncated = false;
    for index in 0..requested_after_lines {
        match read_line(
            &mut buffered,
            limits.max_line_bytes,
            &mut remaining_scan_bytes,
            &mut bytes_scanned,
            None,
            cancellation,
            deadline,
        )? {
            LineRead::Complete(line) => {
                let line_number = reference
                    .line_number
                    .checked_add(
                        u64::try_from(index + 1).map_err(|_| ContextReadError::CounterOverflow)?,
                    )
                    .ok_or(ContextReadError::CounterOverflow)?;
                after_lines.push(line.into_context_line(
                    line_number,
                    false,
                    limits.max_line_bytes,
                    None,
                ));
            }
            LineRead::Eof => break,
            LineRead::BudgetExhausted => {
                after_truncated = true;
                break;
            }
        }
    }

    Ok(ForwardRead {
        match_line,
        after_lines,
        bytes_scanned,
        after_truncated,
    })
}

#[derive(Debug, Clone, Copy)]
struct MatchSpec<'a> {
    relative_offset: u64,
    keyword: &'a [u8],
    case_sensitive: bool,
}

#[derive(Debug)]
struct RawLine {
    preview: Vec<u8>,
    preview_start: u64,
    original_line_bytes: u64,
}

impl RawLine {
    fn into_context_line(
        mut self,
        line_number: u64,
        is_match_line: bool,
        maximum_output_bytes: usize,
        keyword: Option<(&[u8], bool)>,
    ) -> ContextLine {
        let preview_end = self
            .preview_start
            .saturating_add(u64::try_from(self.preview.len()).unwrap_or(u64::MAX));
        let covers_line_end = preview_end == self.original_line_bytes;
        if covers_line_end && self.preview.last() == Some(&b'\r') {
            self.preview.pop();
        }
        let content_lossy = std::str::from_utf8(&self.preview).is_err();
        let converted: Cow<'_, str> = String::from_utf8_lossy(&self.preview);
        let (content, output_truncated) =
            bounded_text(converted.as_ref(), maximum_output_bytes, keyword);

        ContextLine {
            line_number,
            content,
            content_truncated: self.preview_start > 0 || !covers_line_end || output_truncated,
            content_lossy,
            original_line_bytes: self.original_line_bytes,
            is_match_line,
        }
    }
}

enum LineRead {
    Complete(RawLine),
    Eof,
    BudgetExhausted,
}

fn read_line<B: BufRead>(
    reader: &mut B,
    max_line_bytes: usize,
    remaining_scan_bytes: &mut u64,
    bytes_scanned: &mut u64,
    match_spec: Option<MatchSpec<'_>>,
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<LineRead, ContextReadError> {
    let (preview_start, preview_end) = match match_spec {
        Some(spec) => {
            let keyword_len =
                u64::try_from(spec.keyword.len()).map_err(|_| ContextReadError::CounterOverflow)?;
            let before = u64::try_from(max_line_bytes.saturating_sub(spec.keyword.len()) / 2)
                .map_err(|_| ContextReadError::CounterOverflow)?;
            let start = spec.relative_offset.saturating_sub(before);
            let end = start
                .checked_add(
                    u64::try_from(max_line_bytes).map_err(|_| ContextReadError::CounterOverflow)?,
                )
                .ok_or(ContextReadError::CounterOverflow)?;
            if spec.keyword.is_empty() || keyword_len > u64::try_from(max_line_bytes).unwrap_or(0) {
                return Err(ContextReadError::InvalidLimits(
                    "match keyword does not fit in the line preview",
                ));
            }
            (start, end)
        }
        None => (
            0,
            u64::try_from(max_line_bytes).map_err(|_| ContextReadError::CounterOverflow)?,
        ),
    };

    let mut preview = Vec::with_capacity(max_line_bytes);
    let mut original_line_bytes = 0_u64;
    let mut saw_content = false;
    let mut keyword_bytes_checked = 0_usize;
    let mut interrupt_bytes = 0_u64;

    loop {
        check_interrupted(cancellation, deadline)?;
        if *remaining_scan_bytes == 0 {
            if match_spec.is_some() && keyword_bytes_checked < match_spec.unwrap().keyword.len() {
                return Err(ContextReadError::MatchOutsideScanBudget);
            }
            return Ok(LineRead::BudgetExhausted);
        }

        let available = reader.fill_buf()?;
        if available.is_empty() {
            if !saw_content {
                return Ok(LineRead::Eof);
            }
            if let Some(spec) = match_spec
                && keyword_bytes_checked < spec.keyword.len()
            {
                return Err(ContextReadError::FileChanged);
            }
            return Ok(LineRead::Complete(RawLine {
                preview,
                preview_start,
                original_line_bytes,
            }));
        }

        let available_len = available.len();
        let allowed =
            available_len.min(usize::try_from(*remaining_scan_bytes).unwrap_or(usize::MAX));
        let mut consumed = 0_usize;
        let mut completed = false;
        for &byte in &available[..allowed] {
            consumed += 1;
            *remaining_scan_bytes -= 1;
            *bytes_scanned = bytes_scanned
                .checked_add(1)
                .ok_or(ContextReadError::CounterOverflow)?;
            interrupt_bytes += 1;
            if interrupt_bytes >= INTERRUPT_CHECK_INTERVAL_BYTES {
                check_interrupted(cancellation, deadline)?;
                interrupt_bytes = 0;
            }

            if byte == b'\n' {
                completed = true;
                break;
            }
            saw_content = true;
            let position = original_line_bytes;
            original_line_bytes = original_line_bytes
                .checked_add(1)
                .ok_or(ContextReadError::CounterOverflow)?;

            if let Some(spec) = match_spec {
                let keyword_end = spec
                    .relative_offset
                    .checked_add(
                        u64::try_from(spec.keyword.len())
                            .map_err(|_| ContextReadError::CounterOverflow)?,
                    )
                    .ok_or(ContextReadError::CounterOverflow)?;
                if position >= spec.relative_offset && position < keyword_end {
                    let expected = spec.keyword[keyword_bytes_checked];
                    if fold_ascii(byte, spec.case_sensitive)
                        != fold_ascii(expected, spec.case_sensitive)
                    {
                        return Err(ContextReadError::FileChanged);
                    }
                    keyword_bytes_checked += 1;
                }
            }

            if position >= preview_start && position < preview_end {
                preview.push(byte);
            }
        }
        reader.consume(consumed);

        if completed {
            if let Some(spec) = match_spec
                && keyword_bytes_checked < spec.keyword.len()
            {
                return Err(ContextReadError::FileChanged);
            }
            return Ok(LineRead::Complete(RawLine {
                preview,
                preview_start,
                original_line_bytes,
            }));
        }
        if allowed < available_len || *remaining_scan_bytes == 0 {
            if let Some(spec) = match_spec
                && keyword_bytes_checked < spec.keyword.len()
            {
                return Err(ContextReadError::MatchOutsideScanBudget);
            }
            return Ok(LineRead::BudgetExhausted);
        }
    }
}

fn context_line_from_bytes(
    line_number: u64,
    bytes: &[u8],
    maximum_output_bytes: usize,
    is_match_line: bool,
    keyword: Option<(&[u8], bool)>,
) -> ContextLine {
    let original_line_bytes = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    let display_bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
    let preview_len = display_bytes.len().min(maximum_output_bytes);
    let preview = &display_bytes[..preview_len];
    let content_lossy = std::str::from_utf8(preview).is_err();
    let converted = String::from_utf8_lossy(preview);
    let (content, output_truncated) =
        bounded_text(converted.as_ref(), maximum_output_bytes, keyword);
    ContextLine {
        line_number,
        content,
        content_truncated: preview_len < display_bytes.len() || output_truncated,
        content_lossy,
        original_line_bytes,
        is_match_line,
    }
}

fn bounded_text(
    text: &str,
    maximum_bytes: usize,
    keyword: Option<(&[u8], bool)>,
) -> (String, bool) {
    if text.len() <= maximum_bytes {
        return (text.to_owned(), false);
    }

    let (mut start, required_end) = keyword
        .and_then(|(keyword, case_sensitive)| {
            find_folded_subslice(text.as_bytes(), keyword, case_sensitive).map(|offset| {
                (
                    offset.saturating_sub(maximum_bytes.saturating_sub(keyword.len()) / 2),
                    offset + keyword.len(),
                )
            })
        })
        .unwrap_or((0, 0));
    if required_end > start.saturating_add(maximum_bytes) {
        start = required_end.saturating_sub(maximum_bytes);
    }
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let mut end = start.saturating_add(maximum_bytes).min(text.len());
    while end > start && !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[start..end].to_owned(), true)
}

fn find_folded_subslice(haystack: &[u8], needle: &[u8], case_sensitive: bool) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    (0..=haystack.len() - needle.len()).find(|start| {
        haystack[*start..*start + needle.len()]
            .iter()
            .zip(needle)
            .all(|(left, right)| {
                fold_ascii(*left, case_sensitive) == fold_ascii(*right, case_sensitive)
            })
    })
}

fn fold_ascii(byte: u8, case_sensitive: bool) -> u8 {
    if case_sensitive {
        byte
    } else {
        byte.to_ascii_lowercase()
    }
}

fn read_exact_controlled<R: Read>(
    reader: &mut R,
    mut buffer: &mut [u8],
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
    bytes_scanned: &mut u64,
) -> Result<(), ContextReadError> {
    while !buffer.is_empty() {
        check_interrupted(cancellation, deadline)?;
        let chunk_len = buffer.len().min(64 * 1024);
        match reader.read(&mut buffer[..chunk_len]) {
            Ok(0) => return Err(ContextReadError::FileChanged),
            Ok(count) => {
                *bytes_scanned = bytes_scanned
                    .checked_add(
                        u64::try_from(count).map_err(|_| ContextReadError::CounterOverflow)?,
                    )
                    .ok_or(ContextReadError::CounterOverflow)?;
                buffer = &mut buffer[count..];
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(ContextReadError::Io(error)),
        }
    }
    Ok(())
}

fn check_interrupted(
    cancellation: &CancellationToken,
    deadline: Option<Instant>,
) -> Result<(), ContextReadError> {
    if cancellation.is_cancelled() {
        return Err(ContextReadError::Cancelled);
    }
    if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
        return Err(ContextReadError::DeadlineExceeded);
    }
    Ok(())
}

#[derive(Debug, Error)]
pub enum ContextReadError {
    #[error("invalid context request: {0}")]
    InvalidRequest(&'static str),

    #[error("invalid context limits: {0}")]
    InvalidLimits(&'static str),

    #[error("referenced log file changed after the match was created")]
    FileChanged,

    #[error("the match position is outside the bounded context scan budget")]
    MatchOutsideScanBudget,

    #[error("the complete match line exceeds the bounded context scan budget")]
    MatchLineScanLimit,

    #[error("context read was cancelled")]
    Cancelled,

    #[error("context read deadline was exceeded")]
    DeadlineExceeded,

    #[error("context resource counter overflowed")]
    CounterOverflow,

    #[error(transparent)]
    InvalidReference(#[from] QueryStateError),

    #[error(transparent)]
    SourceRegistry(#[from] SourceRegistryError),

    #[error("context file operation failed")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use crate::FileIdentity;

    use super::*;

    fn reference(line_number: u64, line_start: u64, match_offset: u64) -> MatchReferenceData {
        MatchReferenceData {
            source_id: "payment-test".to_owned(),
            file_id: "file_test".to_owned(),
            relative_path: "application.log".into(),
            file_identity: FileIdentity {
                device: 1,
                inode: 2,
            },
            file_size_at_match: 4096,
            line_number,
            line_start_offset: line_start,
            match_byte_offset: match_offset,
            keyword: "MATCH".to_owned(),
            case_sensitive: false,
        }
    }

    fn limits() -> ContextReadLimits {
        ContextReadLimits {
            max_lines_per_side: 50,
            max_line_bytes: 32,
            max_returned_content_bytes: 256,
            max_before_scan_bytes: 1024,
            max_forward_scan_bytes: 1024,
            read_buffer_bytes: 16,
        }
    }

    #[test]
    fn reads_before_match_and_after_lines() {
        let data = b"one\ntwo\nMATCH three\nfour\nfive\n";
        let mut reference = reference(3, 8, 8);
        stale_reference.file_size_at_match = data.len() as u64;
        let mut reader = Cursor::new(data.to_vec());
        let outcome = read_context(
            &mut reader,
            data.len() as u64,
            &reference,
            2,
            2,
            limits(),
            &CancellationToken::new(),
            None,
        )
        .expect("context should be read");

        assert_eq!(outcome.start_line, 1);
        assert_eq!(outcome.end_line, 5);
        assert_eq!(outcome.lines.len(), 5);
        assert_eq!(outcome.lines[2].content, "MATCH three");
        assert!(outcome.lines[2].is_match_line);
        assert!(!outcome.truncated);
    }

    #[test]
    fn long_match_line_preview_contains_keyword() {
        let mut data = vec![b'x'; 100];
        data.extend_from_slice(b"MATCH");
        data.extend(std::iter::repeat_n(b'y', 100));
        data.push(b'\n');
        let mut reference = reference(1, 0, 100);
        reference.file_size_at_match = data.len() as u64;
        let mut reader = Cursor::new(data.clone());
        let outcome = read_context(
            &mut reader,
            data.len() as u64,
            &reference,
            0,
            0,
            limits(),
            &CancellationToken::new(),
            None,
        )
        .expect("context should be read");

        assert!(outcome.lines[0].content.contains("MATCH"));
        assert!(outcome.lines[0].content_truncated);
        assert!(outcome.truncated);
    }

    #[test]
    fn strips_crlf_and_marks_invalid_utf8() {
        let data = [
            b'o', b'n', b'e', b'\r', b'\n', 0xff, b'M', b'A', b'T', b'C', b'H', b'\r', b'\n',
        ];
        let mut reference = reference(2, 5, 6);
        reference.file_size_at_match = data.len() as u64;
        let mut reader = Cursor::new(data);
        let outcome = read_context(
            &mut reader,
            13,
            &reference,
            1,
            0,
            limits(),
            &CancellationToken::new(),
            None,
        )
        .expect("context should be read");

        assert_eq!(outcome.lines[0].content, "one");
        assert!(outcome.lines[1].content_lossy);
        assert!(outcome.lines[1].content.contains("MATCH"));
        assert!(!outcome.lines[1].content.ends_with('\r'));
    }

    #[test]
    fn reports_before_window_truncation() {
        let data = b"old-one\nold-two\nold-three\nMATCH\n";
        let mut reference = reference(4, 26, 26);
        reference.file_size_at_match = data.len() as u64;
        let mut limited = limits();
        limited.max_before_scan_bytes = 10;
        let mut reader = Cursor::new(data.to_vec());
        let outcome = read_context(
            &mut reader,
            data.len() as u64,
            &reference,
            3,
            0,
            limited,
            &CancellationToken::new(),
            None,
        )
        .expect("context should be read");

        assert!(outcome.before_truncated);
        assert!(
            outcome
                .lines
                .last()
                .expect("match line exists")
                .is_match_line
        );
    }

    #[test]
    fn prioritizes_match_line_when_content_budget_is_small() {
        let data = b"before-before\nMATCH\nafter-after\n";
        let mut reference = reference(2, 14, 14);
        reference.file_size_at_match = data.len() as u64;
        let mut limited = limits();
        limited.max_line_bytes = 16;
        limited.max_returned_content_bytes = 16;
        let mut reader = Cursor::new(data.to_vec());
        let outcome = read_context(
            &mut reader,
            data.len() as u64,
            &reference,
            1,
            1,
            limited,
            &CancellationToken::new(),
            None,
        )
        .expect("context should be read");

        assert!(outcome.lines.iter().any(|line| line.is_match_line));
        assert!(outcome.before_truncated || outcome.after_truncated);
        assert!(outcome.returned_content_bytes <= 16);
    }

    #[test]
    fn detects_rewritten_keyword_and_cancelled_request() {
        let data = b"prefix changed suffix\n";
        let mut stale_reference = reference(1, 0, 7);
        stale_reference.file_size_at_match = data.len() as u64;
        let mut reader = Cursor::new(data.to_vec());
        assert!(matches!(
            read_context(
                &mut reader,
                data.len() as u64,
                &stale_reference,
                0,
                0,
                limits(),
                &CancellationToken::new(),
                None,
            ),
            Err(ContextReadError::FileChanged)
        ));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let mut valid_reference = reference(1, 0, 0);
        valid_reference.file_size_at_match = 6;
        let mut reader = Cursor::new(b"MATCH\n".to_vec());
        assert!(matches!(
            read_context(
                &mut reader,
                6,
                &valid_reference,
                0,
                0,
                limits(),
                &cancellation,
                None,
            ),
            Err(ContextReadError::Cancelled)
        ));
    }
}
