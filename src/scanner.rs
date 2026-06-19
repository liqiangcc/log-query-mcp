use std::{
    collections::VecDeque,
    io::{ErrorKind, Read},
    time::Instant,
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

const INTERRUPT_CHECK_INTERVAL_BYTES: u64 = 4 * 1024;
pub const MAX_READ_BUFFER_BYTES: usize = 1024 * 1024;
pub const MAX_LINE_PREVIEW_BYTES: usize = 1024 * 1024;
pub const MAX_RETURNED_CONTENT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_SCAN_RESULTS: usize = 200;

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
            max_returned_content_bytes: 1024 * 1024,
            read_buffer_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanRequest {
    pub keyword: String,
    pub case_sensitive: bool,
    pub limits: ScanLimits,
    pub deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl ScanRequest {
    #[must_use]
    pub fn new(keyword: impl Into<String>) -> Self {
        Self {
            keyword: keyword.into(),
            case_sensitive: false,
            limits: ScanLimits::default(),
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
    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    fn validate(&self) -> Result<(), ScanError> {
        let keyword_bytes = self.keyword.as_bytes();
        if keyword_bytes.is_empty()
            || keyword_bytes.contains(&b'\n')
            || keyword_bytes.contains(&b'\r')
        {
            return Err(ScanError::InvalidKeyword);
        }
        if self.limits.max_scan_bytes == 0 {
            return Err(ScanError::InvalidLimits(
                "max_scan_bytes must be greater than zero",
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
                "max_returned_content_bytes must be between 1 and 16777216",
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
    pub results: Vec<ScanMatch>,
    pub bytes_scanned: u64,
    pub lines_scanned: u64,
    pub returned_content_bytes: usize,
    pub stop_reason: ScanStopReason,
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("keyword must be non-empty and cannot contain line separators")]
    InvalidKeyword,

    #[error("invalid scan limits: {0}")]
    InvalidLimits(&'static str),

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
    fn new(pre_match_window_capacity: usize, max_line_bytes: usize) -> Self {
        Self {
            line_number: 1,
            line_start_offset: 0,
            line_bytes: 0,
            matcher_state: 0,
            pre_match_window: VecDeque::with_capacity(pre_match_window_capacity),
            pre_match_window_capacity,
            preview: Vec::with_capacity(max_line_bytes),
            preview_start_in_line: 0,
            match_byte_offset: None,
        }
    }

    fn has_bytes(&self) -> bool {
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

    fn reset(&mut self, line_number: u64, line_start_offset: u64) {
        self.line_number = line_number;
        self.line_start_offset = line_start_offset;
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
    let mut line = LineCapture::new(pre_match_window_capacity, request.limits.max_line_bytes);
    let mut buffer = vec![0_u8; request.limits.read_buffer_bytes];
    let mut results = Vec::with_capacity(request.limits.max_results);
    let mut bytes_scanned = 0_u64;
    let mut lines_scanned = 0_u64;
    let mut returned_content_bytes = 0_usize;
    let mut bytes_since_interrupt_check = 0_u64;

    loop {
        if let Some(reason) = interrupt_reason(request) {
            return Ok(outcome(
                results,
                bytes_scanned,
                lines_scanned,
                returned_content_bytes,
                reason,
            ));
        }
        if bytes_scanned >= request.limits.max_scan_bytes {
            return Ok(outcome(
                results,
                bytes_scanned,
                lines_scanned,
                returned_content_bytes,
                ScanStopReason::ScanByteLimit,
            ));
        }

        let remaining = request.limits.max_scan_bytes - bytes_scanned;
        let read_size = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let bytes_read = match reader.read(&mut buffer[..read_size]) {
            Ok(0) => {
                if line.has_bytes() {
                    lines_scanned += 1;
                    if let Some(reason) = finish_line(
                        &mut line,
                        &mut results,
                        &mut returned_content_bytes,
                        request.limits,
                    ) {
                        return Ok(outcome(
                            results,
                            bytes_scanned,
                            lines_scanned,
                            returned_content_bytes,
                            reason,
                        ));
                    }
                }
                return Ok(outcome(
                    results,
                    bytes_scanned,
                    lines_scanned,
                    returned_content_bytes,
                    ScanStopReason::Complete,
                ));
            }
            Ok(bytes_read) => bytes_read,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(ScanError::Io(error)),
        };

        for &byte in &buffer[..bytes_read] {
            if bytes_since_interrupt_check >= INTERRUPT_CHECK_INTERVAL_BYTES {
                if let Some(reason) = interrupt_reason(request) {
                    return Ok(outcome(
                        results,
                        bytes_scanned,
                        lines_scanned,
                        returned_content_bytes,
                        reason,
                    ));
                }
                bytes_since_interrupt_check = 0;
            }

            let absolute_offset = bytes_scanned;
            bytes_scanned += 1;
            bytes_since_interrupt_check += 1;

            if byte == b'\n' {
                lines_scanned += 1;
                if let Some(reason) = finish_line(
                    &mut line,
                    &mut results,
                    &mut returned_content_bytes,
                    request.limits,
                ) {
                    return Ok(outcome(
                        results,
                        bytes_scanned,
                        lines_scanned,
                        returned_content_bytes,
                        reason,
                    ));
                }
                line.reset(lines_scanned + 1, bytes_scanned);
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

fn outcome(
    results: Vec<ScanMatch>,
    bytes_scanned: u64,
    lines_scanned: u64,
    returned_content_bytes: usize,
    stop_reason: ScanStopReason,
) -> ScanOutcome {
    ScanOutcome {
        results,
        bytes_scanned,
        lines_scanned,
        returned_content_bytes,
        stop_reason,
    }
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
    fn matches_utf8_keyword() {
        let outcome = scan(
            "请求失败 orderId=10001\n".as_bytes(),
            ScanRequest::new("失败"),
        )
        .expect("scan should succeed");

        assert_eq!(outcome.results.len(), 1);
        assert!(outcome.results[0].content.contains("失败"));
    }

    #[test]
    fn performs_ascii_case_insensitive_matching_by_default() {
        let outcome = scan(
            b"PaymentAuthException\n",
            ScanRequest::new("paymentauthexception"),
        )
        .expect("scan should succeed");

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

        assert_eq!(outcome.results.len(), 1);
        let result = &outcome.results[0];
        assert!(result.content.contains("MATCH"));
        assert!(result.content.len() <= 16);
        assert!(result.content_truncated);
        assert_eq!(result.original_line_bytes, 41);
    }

    #[test]
    fn reports_lossy_preview_for_invalid_utf8() {
        let data = [0xff, b' ', b'M', b'A', b'T', b'C', b'H', b'\n'];
        let outcome = scan(&data, ScanRequest::new("MATCH")).expect("scan should succeed");

        assert_eq!(outcome.results.len(), 1);
        assert!(outcome.results[0].content_lossy);
        assert!(outcome.results[0].content.contains("MATCH"));
    }

    #[test]
    fn strips_carriage_return_from_complete_crlf_line() {
        let outcome =
            scan(b"traceId=abc123\r\n", ScanRequest::new("abc123")).expect("scan should succeed");

        assert_eq!(outcome.results[0].content, "traceId=abc123");
    }

    #[test]
    fn stops_at_result_limit() {
        let limits = ScanLimits {
            max_results: 2,
            ..ScanLimits::default()
        };
        let outcome = scan(
            b"MATCH one\nMATCH two\nMATCH three\n",
            ScanRequest::new("MATCH").with_limits(limits),
        )
        .expect("scan should succeed");

        assert_eq!(outcome.results.len(), 2);
        assert_eq!(outcome.stop_reason, ScanStopReason::ResultLimit);
    }

    #[test]
    fn stops_at_scan_byte_limit_without_emitting_partial_line() {
        let limits = ScanLimits {
            max_scan_bytes: 4,
            ..ScanLimits::default()
        };
        let outcome = scan(b"abcdef\n", ScanRequest::new("def").with_limits(limits))
            .expect("scan should succeed");

        assert!(outcome.results.is_empty());
        assert_eq!(outcome.bytes_scanned, 4);
        assert_eq!(outcome.stop_reason, ScanStopReason::ScanByteLimit);
    }

    #[test]
    fn stops_before_exceeding_returned_content_limit() {
        let limits = ScanLimits {
            max_returned_content_bytes: 3,
            ..ScanLimits::default()
        };
        let outcome = scan(b"MATCH\n", ScanRequest::new("MATCH").with_limits(limits))
            .expect("scan should succeed");

        assert!(outcome.results.is_empty());
        assert_eq!(
            outcome.stop_reason,
            ScanStopReason::ReturnedContentByteLimit
        );
    }

    #[test]
    fn honors_pre_cancelled_request() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let request = ScanRequest::new("MATCH").with_cancellation(cancellation);
        let outcome = scan(b"MATCH\n", request).expect("scan should succeed");

        assert_eq!(outcome.bytes_scanned, 0);
        assert_eq!(outcome.stop_reason, ScanStopReason::Cancelled);
    }

    #[test]
    fn honors_expired_deadline() {
        let deadline = Instant::now() - Duration::from_millis(1);
        let outcome = scan(
            b"MATCH\n",
            ScanRequest::new("MATCH").with_deadline(deadline),
        )
        .expect("scan should succeed");

        assert_eq!(outcome.bytes_scanned, 0);
        assert_eq!(outcome.stop_reason, ScanStopReason::DeadlineExceeded);
    }

    #[test]
    fn rejects_invalid_keyword_and_limits() {
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
    }
}
