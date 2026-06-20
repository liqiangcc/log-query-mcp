use std::{
    collections::VecDeque,
    io::{ErrorKind, Read},
    time::Instant,
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

const INTERRUPT_CHECK_INTERVAL_BYTES: u64 = 4 * 1024;
pub const MAX_SCAN_KEYWORD_CHARS: usize = 256;
pub const MAX_READ_BUFFER_BYTES: usize = 1024 * 1024;
pub const MAX_LINE_PREVIEW_BYTES: usize = 1024 * 1024;
pub const MAX_RETURNED_CONTENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SCAN_RESULTS: usize = 200;
pub const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanPosition {
    pub byte_offset: u64,
    pub line_number: u64,
}

impl Default for ScanPosition {
    fn default() -> Self {
        Self {
            byte_offset: 0,
            line_number: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanLimits {
    pub max_scan_bytes: u64,
    pub max_results: usize,
    pub max_line_bytes: usize,
    pub max_returned_content_bytes: usize,
    pub read_buffer_bytes: usize,
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_scan_bytes: 512 * 1024 * 1024,
            max_results: 50,
            max_line_bytes: 16 * 1024,
            max_returned_content_bytes: 512 * 1024,
            read_buffer_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanRequest {
    keyword: String,
    case_sensitive: bool,
    limits: ScanLimits,
    start_position: ScanPosition,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl ScanRequest {
    #[must_use]
    pub fn new(keyword: impl Into<String>) -> Self {
        Self {
            keyword: keyword.into(),
            case_sensitive: false,
            limits: ScanLimits::default(),
            start_position: ScanPosition::default(),
            deadline: None,
            cancellation: CancellationToken::new(),
        }
    }

    #[must_use]
    pub fn with_case_sensitive(mut self, case_sensitive: bool) -> Self {
        self.case_sensitive = case_sensitive;
        self
    }

    #[must_use]
    pub fn with_limits(mut self, limits: ScanLimits) -> Self {
        self.limits = limits;
        self
    }

    #[must_use]
    pub fn with_start_position(mut self, start_position: ScanPosition) -> Self {
        self.start_position = start_position;
        self
    }

    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    #[must_use]
    pub fn with_cancellation(mut self, cancellation: CancellationToken) -> Self {
        self.cancellation = cancellation;
        self
    }

    #[must_use]
    pub fn keyword(&self) -> &str {
        &self.keyword
    }

    #[must_use]
    pub const fn case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    #[must_use]
    pub const fn limits(&self) -> ScanLimits {
        self.limits
    }

    #[must_use]
    pub const fn start_position(&self) -> ScanPosition {
        self.start_position
    }

    #[must_use]
    pub const fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    #[must_use]
    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    fn validate(&self) -> Result<(), ScanError> {
        let keyword_bytes = self.keyword.as_bytes();
        let keyword_chars = self.keyword.chars().count();
        if keyword_chars == 0
            || keyword_chars > MAX_SCAN_KEYWORD_CHARS
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(ScanError::InvalidKeyword);
        }
        if self.start_position.line_number == 0 {
            return Err(ScanError::InvalidStartPosition);
        }
        if self.limits.max_scan_bytes == 0 || self.limits.max_scan_bytes > MAX_SCAN_BYTES {
            return Err(ScanError::InvalidLimits(
                "max_scan_bytes is outside the service limit",
            ));
        }
        if self.limits.max_results == 0 || self.limits.max_results > MAX_SCAN_RESULTS {
            return Err(ScanError::InvalidLimits(
                "max_results must be between 1 and 200",
            ));
        }
        if self.limits.max_line_bytes == 0 || self.limits.max_line_bytes > MAX_LINE_PREVIEW_BYTES {
            return Err(ScanError::InvalidLimits(
                "max_line_bytes must be between 1 and 1048576",
            ));
        }
        if keyword_bytes.len() > self.limits.max_line_bytes {
            return Err(ScanError::InvalidLimits(
                "max_line_bytes must be at least the keyword byte length",
            ));
        }
        if self.limits.max_returned_content_bytes == 0
            || self.limits.max_returned_content_bytes > MAX_RETURNED_CONTENT_BYTES
        {
            return Err(ScanError::InvalidLimits(
                "max_returned_content_bytes is outside the service limit",
            ));
        }
        if self.limits.read_buffer_bytes == 0
            || self.limits.read_buffer_bytes > MAX_READ_BUFFER_BYTES
        {
            return Err(ScanError::InvalidLimits(
                "read_buffer_bytes must be between 1 and 1048576",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanMatch {
    pub line_number: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
    pub content: String,
    pub content_truncated: bool,
    pub content_lossy: bool,
    pub original_line_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanStopReason {
    Complete,
    ResultLimit,
    ScanByteLimit,
    ReturnedContentByteLimit,
    Cancelled,
    DeadlineExceeded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanOutcome {
    pub start_position: ScanPosition,
    pub next_position: Option<ScanPosition>,
    pub results: Vec<ScanMatch>,
    pub bytes_scanned: u64,
    pub lines_scanned: u64,
    pub returned_content_bytes: usize,
    pub stop_reason: ScanStopReason,
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("keyword must contain 1 to 256 characters and no line separators")]
    InvalidKeyword,

    #[error("scan start line number must be at least one")]
    InvalidStartPosition,

    #[error("invalid scan limits: {0}")]
    InvalidLimits(&'static str),

    #[error("scan byte or line position overflowed")]
    PositionOverflow,

    #[error("log read failed")]
    Io(#[from] std::io::Error),
}

#[derive(Debug)]
struct LineCapture {
    line_number: u64,
    line_start_offset: u64,
    line_bytes: u64,
    matcher_state: usize,
    pre_match_window: VecDeque<u8>,
    pre_match_window_capacity: usize,
    preview: Vec<u8>,
    preview_start_in_line: u64,
    match_byte_offset: Option<u64>,
}

impl LineCapture {
    fn new(
        position: ScanPosition,
        pre_match_window_capacity: usize,
        max_line_bytes: usize,
    ) -> Self {
        Self {
            line_number: position.line_number,
            line_start_offset: position.byte_offset,
            line_bytes: 0,
            matcher_state: 0,
            pre_match_window: VecDeque::with_capacity(pre_match_window_capacity),
            pre_match_window_capacity,
            preview: Vec::with_capacity(max_line_bytes),
            preview_start_in_line: 0,
            match_byte_offset: None,
        }
    }

    const fn has_bytes(&self) -> bool {
        self.line_bytes > 0
    }

    fn push_byte(
        &mut self,
        byte: u8,
        absolute_offset: u64,
        pattern: &[u8],
        failure: &[usize],
        case_sensitive: bool,
        max_line_bytes: usize,
    ) {
        self.line_bytes += 1;

        if self.match_byte_offset.is_some() {
            if self.preview.len() < max_line_bytes {
                self.preview.push(byte);
            }
            return;
        }

        if self.pre_match_window.len() == self.pre_match_window_capacity {
            self.pre_match_window.pop_front();
        }
        self.pre_match_window.push_back(byte);

        let candidate = fold_ascii(byte, case_sensitive);
        while self.matcher_state > 0 && pattern[self.matcher_state] != candidate {
            self.matcher_state = failure[self.matcher_state - 1];
        }
        if pattern[self.matcher_state] == candidate {
            self.matcher_state += 1;
        }

        if self.matcher_state == pattern.len() {
            let pattern_len = u64::try_from(pattern.len()).expect("pattern length fits in u64");
            let window_len =
                u64::try_from(self.pre_match_window.len()).expect("window length fits in u64");
            self.match_byte_offset = Some(absolute_offset + 1 - pattern_len);
            self.preview_start_in_line = self.line_bytes - window_len;
            self.preview.extend(self.pre_match_window.iter().copied());
            self.matcher_state = failure[self.matcher_state - 1];
        }
    }

    fn take_match(&mut self) -> Option<ScanMatch> {
        let match_byte_offset = self.match_byte_offset?;
        let preview_end_in_line = self.preview_start_in_line
            + u64::try_from(self.preview.len()).expect("preview length fits in u64");
        let covers_line_end = preview_end_in_line == self.line_bytes;

        if covers_line_end && self.preview.last() == Some(&b'\r') {
            self.preview.pop();
        }

        let content_truncated = self.preview_start_in_line > 0 || !covers_line_end;
        let (content, content_lossy) = match String::from_utf8(self.preview.clone()) {
            Ok(content) => (content, false),
            Err(error) => (String::from_utf8_lossy(error.as_bytes()).into_owned(), true),
        };

        Some(ScanMatch {
            line_number: self.line_number,
            line_start_offset: self.line_start_offset,
            match_byte_offset,
            content,
            content_truncated,
            content_lossy,
            original_line_bytes: self.line_bytes,
        })
    }

    fn reset(&mut self, position: ScanPosition) {
        self.line_number = position.line_number;
        self.line_start_offset = position.byte_offset;
        self.line_bytes = 0;
        self.matcher_state = 0;
        self.pre_match_window.clear();
        self.preview.clear();
        self.preview_start_in_line = 0;
        self.match_byte_offset = None;
    }
}

pub fn scan_reader<R: Read>(
    reader: &mut R,
    request: &ScanRequest,
) -> Result<ScanOutcome, ScanError> {
    request.validate()?;

    let pattern: Vec<u8> = request
        .keyword
        .as_bytes()
        .iter()
        .map(|byte| fold_ascii(*byte, request.case_sensitive))
        .collect();
    let failure = build_failure_table(&pattern);
    let before_context = (request.limits.max_line_bytes - pattern.len()) / 2;
    let pre_match_window_capacity = before_context + pattern.len();
    let mut line = LineCapture::new(
        request.start_position,
        pre_match_window_capacity,
        request.limits.max_line_bytes,
    );
    let mut buffer = vec![0_u8; request.limits.read_buffer_bytes];
    let mut results = Vec::with_capacity(request.limits.max_results);
    let mut bytes_scanned = 0_u64;
    let mut lines_scanned = 0_u64;
    let mut returned_content_bytes = 0_usize;
    let mut bytes_since_interrupt_check = 0_u64;

    loop {
        if let Some(reason) = interrupt_reason(request) {
            return build_outcome(
                request,
                results,
                bytes_scanned,
                lines_scanned,
                returned_content_bytes,
                reason,
                boundary_position(request, bytes_scanned, lines_scanned, &line)?,
            );
        }
        if bytes_scanned >= request.limits.max_scan_bytes {
            return build_outcome(
                request,
                results,
                bytes_scanned,
                lines_scanned,
                returned_content_bytes,
                ScanStopReason::ScanByteLimit,
                boundary_position(request, bytes_scanned, lines_scanned, &line)?,
            );
        }

        let remaining = request.limits.max_scan_bytes - bytes_scanned;
        let read_size = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let bytes_read = match reader.read(&mut buffer[..read_size]) {
            Ok(0) => {
                if line.has_bytes() {
                    lines_scanned = lines_scanned
                        .checked_add(1)
                        .ok_or(ScanError::PositionOverflow)?;
                    if let Some(reason) = finish_line(
                        &mut line,
                        &mut results,
                        &mut returned_content_bytes,
                        request.limits,
                    ) {
                        return build_outcome(
                            request,
                            results,
                            bytes_scanned,
                            lines_scanned,
                            returned_content_bytes,
                            reason,
                            None,
                        );
                    }
                }
                return build_outcome(
                    request,
                    results,
                    bytes_scanned,
                    lines_scanned,
                    returned_content_bytes,
                    ScanStopReason::Complete,
                    None,
                );
            }
            Ok(bytes_read) => bytes_read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(ScanError::Io(error)),
        };

        for &byte in &buffer[..bytes_read] {
            if bytes_since_interrupt_check >= INTERRUPT_CHECK_INTERVAL_BYTES {
                if let Some(reason) = interrupt_reason(request) {
                    return build_outcome(
                        request,
                        results,
                        bytes_scanned,
                        lines_scanned,
                        returned_content_bytes,
                        reason,
                        boundary_position(request, bytes_scanned, lines_scanned, &line)?,
                    );
                }
                bytes_since_interrupt_check = 0;
            }

            let absolute_offset = request
                .start_position
                .byte_offset
                .checked_add(bytes_scanned)
                .ok_or(ScanError::PositionOverflow)?;
            bytes_scanned = bytes_scanned
                .checked_add(1)
                .ok_or(ScanError::PositionOverflow)?;
            bytes_since_interrupt_check += 1;

            if byte == b'\n' {
                lines_scanned = lines_scanned
                    .checked_add(1)
                    .ok_or(ScanError::PositionOverflow)?;
                let next_position = position_after_lines(request, bytes_scanned, lines_scanned)?;
                if let Some(reason) = finish_line(
                    &mut line,
                    &mut results,
                    &mut returned_content_bytes,
                    request.limits,
                ) {
                    return build_outcome(
                        request,
                        results,
                        bytes_scanned,
                        lines_scanned,
                        returned_content_bytes,
                        reason,
                        Some(next_position),
                    );
                }
                line.reset(next_position);
                continue;
            }

            line.push_byte(
                byte,
                absolute_offset,
                &pattern,
                &failure,
                request.case_sensitive,
                request.limits.max_line_bytes,
            );
        }
    }
}

fn finish_line(
    line: &mut LineCapture,
    results: &mut Vec<ScanMatch>,
    returned_content_bytes: &mut usize,
    limits: ScanLimits,
) -> Option<ScanStopReason> {
    let scan_match = line.take_match()?;
    let content_bytes = scan_match.content.len();

    if returned_content_bytes.saturating_add(content_bytes) > limits.max_returned_content_bytes {
        return Some(ScanStopReason::ReturnedContentByteLimit);
    }

    *returned_content_bytes += content_bytes;
    results.push(scan_match);

    if results.len() >= limits.max_results {
        return Some(ScanStopReason::ResultLimit);
    }
    if *returned_content_bytes >= limits.max_returned_content_bytes {
        return Some(ScanStopReason::ReturnedContentByteLimit);
    }
    None
}

fn build_outcome(
    request: &ScanRequest,
    results: Vec<ScanMatch>,
    bytes_scanned: u64,
    lines_scanned: u64,
    returned_content_bytes: usize,
    stop_reason: ScanStopReason,
    next_position: Option<ScanPosition>,
) -> Result<ScanOutcome, ScanError> {
    Ok(ScanOutcome {
        start_position: request.start_position,
        next_position,
        results,
        bytes_scanned,
        lines_scanned,
        returned_content_bytes,
        stop_reason,
    })
}

fn boundary_position(
    request: &ScanRequest,
    bytes_scanned: u64,
    lines_scanned: u64,
    line: &LineCapture,
) -> Result<Option<ScanPosition>, ScanError> {
    if line.has_bytes() {
        Ok(None)
    } else {
        position_after_lines(request, bytes_scanned, lines_scanned).map(Some)
    }
}

fn position_after_lines(
    request: &ScanRequest,
    bytes_scanned: u64,
    lines_scanned: u64,
) -> Result<ScanPosition, ScanError> {
    Ok(ScanPosition {
        byte_offset: request
            .start_position
            .byte_offset
            .checked_add(bytes_scanned)
            .ok_or(ScanError::PositionOverflow)?,
        line_number: request
            .start_position
            .line_number
            .checked_add(lines_scanned)
            .ok_or(ScanError::PositionOverflow)?,
    })
}

fn interrupt_reason(request: &ScanRequest) -> Option<ScanStopReason> {
    if request.cancellation.is_cancelled() {
        return Some(ScanStopReason::Cancelled);
    }
    if request
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        return Some(ScanStopReason::DeadlineExceeded);
    }
    None
}

fn fold_ascii(byte: u8, case_sensitive: bool) -> u8 {
    if case_sensitive {
        byte
    } else {
        byte.to_ascii_lowercase()
    }
}

fn build_failure_table(pattern: &[u8]) -> Vec<usize> {
    let mut failure = vec![0; pattern.len()];
    let mut prefix_len = 0;

    for index in 1..pattern.len() {
        while prefix_len > 0 && pattern[index] != pattern[prefix_len] {
            prefix_len = failure[prefix_len - 1];
        }
        if pattern[index] == pattern[prefix_len] {
            prefix_len += 1;
            failure[index] = prefix_len;
        }
    }
    failure
}

#[cfg(test)]
mod tests {
    use std::{io::Cursor, time::Duration};

    use super::*;

    fn scan(data: &[u8], request: ScanRequest) -> Result<ScanOutcome, ScanError> {
        scan_reader(&mut Cursor::new(data), &request)
    }

    #[test]
    fn matches_keyword_across_read_buffer_boundaries() {
        let limits = ScanLimits {
            read_buffer_bytes: 2,
            ..ScanLimits::default()
        };
        let outcome = scan(b"xxabcxx\n", ScanRequest::new("abc").with_limits(limits))
            .expect("scan should succeed");

        assert_eq!(outcome.results.len(), 1);
        assert_eq!(outcome.results[0].match_byte_offset, 2);
        assert_eq!(outcome.stop_reason, ScanStopReason::Complete);
    }

    #[test]
    fn matches_utf8_keyword_and_ascii_case_insensitive_text() {
        let outcome = scan(
            "请求失败 PaymentAuthException\n".as_bytes(),
            ScanRequest::new("失败"),
        )
        .expect("UTF-8 scan should succeed");
        assert_eq!(outcome.results.len(), 1);

        let outcome = scan(
            b"PaymentAuthException\n",
            ScanRequest::new("paymentauthexception"),
        )
        .expect("ASCII folded scan should succeed");
        assert_eq!(outcome.results.len(), 1);
    }

    #[test]
    fn does_not_match_across_line_boundaries() {
        let outcome = scan(b"ab\nc\n", ScanRequest::new("abc")).expect("scan should succeed");

        assert!(outcome.results.is_empty());
        assert_eq!(outcome.lines_scanned, 2);
    }

    #[test]
    fn returns_bounded_preview_containing_late_match() {
        let limits = ScanLimits {
            max_line_bytes: 16,
            ..ScanLimits::default()
        };
        let data = b"aaaaaaaaaaaaaaaaaaaaMATCHbbbbbbbbbbbbbbbb\n";
        let outcome =
            scan(data, ScanRequest::new("MATCH").with_limits(limits)).expect("scan should succeed");

        let result = &outcome.results[0];
        assert!(result.content.contains("MATCH"));
        assert!(result.content.len() <= 16);
        assert!(result.content_truncated);
        assert_eq!(result.original_line_bytes, 41);
    }

    #[test]
    fn reports_lossy_preview_and_strips_crlf() {
        let invalid = [0xff, b' ', b'M', b'A', b'T', b'C', b'H', b'\n'];
        let outcome = scan(&invalid, ScanRequest::new("MATCH")).expect("scan should succeed");
        assert!(outcome.results[0].content_lossy);

        let outcome = scan(b"traceId=abc123\r\n", ScanRequest::new("abc123"))
            .expect("CRLF scan should succeed");
        assert_eq!(outcome.results[0].content, "traceId=abc123");
    }

    #[test]
    fn result_limit_returns_safe_continuation_position() {
        let limits = ScanLimits {
            max_results: 1,
            ..ScanLimits::default()
        };
        let outcome = scan(
            b"MATCH one\nMATCH two\n",
            ScanRequest::new("MATCH").with_limits(limits),
        )
        .expect("scan should succeed");

        assert_eq!(outcome.stop_reason, ScanStopReason::ResultLimit);
        assert_eq!(
            outcome.next_position,
            Some(ScanPosition {
                byte_offset: 10,
                line_number: 2,
            })
        );
    }

    #[test]
    fn scan_byte_limit_only_returns_continuation_at_line_boundary() {
        let mid_line_limits = ScanLimits {
            max_scan_bytes: 4,
            ..ScanLimits::default()
        };
        let outcome = scan(
            b"abcdef\n",
            ScanRequest::new("def").with_limits(mid_line_limits),
        )
        .expect("scan should succeed");
        assert_eq!(outcome.stop_reason, ScanStopReason::ScanByteLimit);
        assert_eq!(outcome.next_position, None);

        let boundary_limits = ScanLimits {
            max_scan_bytes: 4,
            ..ScanLimits::default()
        };
        let outcome = scan(
            b"abc\nnext\n",
            ScanRequest::new("never").with_limits(boundary_limits),
        )
        .expect("scan should succeed");
        assert_eq!(
            outcome.next_position,
            Some(ScanPosition {
                byte_offset: 4,
                line_number: 2,
            })
        );
    }

    #[test]
    fn resumed_scan_uses_absolute_offsets_and_line_numbers() {
        let outcome = scan(
            b"xxMATCH\n",
            ScanRequest::new("MATCH").with_start_position(ScanPosition {
                byte_offset: 100,
                line_number: 42,
            }),
        )
        .expect("scan should succeed");

        assert_eq!(outcome.results[0].line_number, 42);
        assert_eq!(outcome.results[0].line_start_offset, 100);
        assert_eq!(outcome.results[0].match_byte_offset, 102);
    }

    #[test]
    fn honors_pre_cancelled_request_and_expired_deadline() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let outcome = scan(
            b"MATCH\n",
            ScanRequest::new("MATCH").with_cancellation(cancellation),
        )
        .expect("cancelled scan should return an outcome");
        assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
        assert_eq!(outcome.bytes_scanned, 0);

        let deadline = Instant::now() - Duration::from_millis(1);
        let outcome = scan(
            b"MATCH\n",
            ScanRequest::new("MATCH").with_deadline(deadline),
        )
        .expect("deadline scan should return an outcome");
        assert_eq!(outcome.stop_reason, ScanStopReason::DeadlineExceeded);
    }

    #[test]
    fn rejects_invalid_keyword_limits_and_position() {
        assert!(matches!(
            scan(b"data", ScanRequest::new("")),
            Err(ScanError::InvalidKeyword)
        ));
        assert!(matches!(
            scan(b"data", ScanRequest::new("a\nb")),
            Err(ScanError::InvalidKeyword)
        ));

        let limits = ScanLimits {
            max_line_bytes: 2,
            ..ScanLimits::default()
        };
        assert!(matches!(
            scan(b"MATCH", ScanRequest::new("MATCH").with_limits(limits)),
            Err(ScanError::InvalidLimits(_))
        ));
        assert!(matches!(
            scan(
                b"MATCH",
                ScanRequest::new("MATCH").with_start_position(ScanPosition {
                    byte_offset: 0,
                    line_number: 0,
                })
            ),
            Err(ScanError::InvalidStartPosition)
        ));
    }
}
