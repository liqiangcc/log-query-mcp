use std::{collections::VecDeque, io::Read, time::Instant};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::LimitsConfig;

const CHECK_INTERVAL_BYTES: u64 = 4 * 1024;
const DEFAULT_BUFFER_BYTES: usize = 64 * 1024;
pub const MAX_SCAN_BYTES: u64 = 64 * 1024 * 1024 * 1024;
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

impl ScanLimits {
    pub fn from_service_limits(
        limits: &LimitsConfig,
        max_results: usize,
    ) -> Result<Self, ScanError> {
        if max_results == 0 || max_results > limits.max_results_per_page {
            return Err(ScanError::InvalidLimits(
                "max_results exceeds service limit",
            ));
        }
        let value = Self {
            max_scan_bytes: limits.max_scan_bytes_per_page,
            max_results,
            max_line_bytes: limits.max_line_bytes,
            max_returned_content_bytes: limits.max_returned_content_bytes,
            read_buffer_bytes: DEFAULT_BUFFER_BYTES,
        };
        value.validate(&[])?;
        Ok(value)
    }

    fn validate(self, keyword: &[u8]) -> Result<(), ScanError> {
        if self.max_scan_bytes == 0 || self.max_scan_bytes > MAX_SCAN_BYTES {
            return Err(ScanError::InvalidLimits("invalid max_scan_bytes"));
        }
        if self.max_results == 0 || self.max_results > MAX_SCAN_RESULTS {
            return Err(ScanError::InvalidLimits("invalid max_results"));
        }
        if self.max_line_bytes == 0 || self.max_line_bytes > MAX_LINE_PREVIEW_BYTES {
            return Err(ScanError::InvalidLimits("invalid max_line_bytes"));
        }
        if keyword.len() > self.max_line_bytes {
            return Err(ScanError::InvalidLimits("keyword exceeds max_line_bytes"));
        }
        if self.max_returned_content_bytes == 0
            || self.max_returned_content_bytes > MAX_RETURNED_CONTENT_BYTES
        {
            return Err(ScanError::InvalidLimits("invalid returned content limit"));
        }
        if self.read_buffer_bytes == 0 || self.read_buffer_bytes > MAX_READ_BUFFER_BYTES {
            return Err(ScanError::InvalidLimits("invalid read buffer size"));
        }
        Ok(())
    }
}

impl Default for ScanLimits {
    fn default() -> Self {
        Self {
            max_scan_bytes: 512 * 1024 * 1024,
            max_results: 50,
            max_line_bytes: 16 * 1024,
            max_returned_content_bytes: 512 * 1024,
            read_buffer_bytes: DEFAULT_BUFFER_BYTES,
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
    pub fn with_case_sensitive(mut self, value: bool) -> Self {
        self.case_sensitive = value;
        self
    }

    #[must_use]
    pub fn with_limits(mut self, value: ScanLimits) -> Self {
        self.limits = value;
        self
    }

    #[must_use]
    pub fn with_deadline(mut self, value: Instant) -> Self {
        self.deadline = Some(value);
        self
    }

    #[must_use]
    pub fn with_cancellation(mut self, value: CancellationToken) -> Self {
        self.cancellation = value;
        self
    }

    #[must_use]
    pub const fn cancellation(&self) -> &CancellationToken {
        &self.cancellation
    }

    pub fn validate(&self) -> Result<(), ScanError> {
        let keyword = self.keyword.as_bytes();
        if keyword.is_empty() || keyword.contains(&b'\n') || keyword.contains(&b'\r') {
            return Err(ScanError::InvalidKeyword);
        }
        self.limits.validate(keyword)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanMatchPosition {
    pub line_number: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
    pub original_line_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanMatch {
    pub position: ScanMatchPosition,
    pub content: String,
    pub content_truncated: bool,
    pub content_lossy: bool,
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

impl ScanOutcome {
    #[must_use]
    pub fn stopped_by_limit(&self) -> bool {
        matches!(
            self.stop_reason,
            ScanStopReason::ResultLimit
                | ScanStopReason::ScanByteLimit
                | ScanStopReason::ReturnedContentByteLimit
        )
    }
}

#[derive(Debug, Error)]
pub enum ScanError {
    #[error("keyword must be non-empty and contain no line separators")]
    InvalidKeyword,
    #[error("invalid scan limits: {0}")]
    InvalidLimits(&'static str),
    #[error("log read failed")]
    Io(#[from] std::io::Error),
}

struct LineState {
    number: u64,
    start: u64,
    bytes: u64,
    matcher: usize,
    before: VecDeque<u8>,
    before_capacity: usize,
    preview: Vec<u8>,
    preview_start: u64,
    match_offset: Option<u64>,
}

impl LineState {
    fn new(before_capacity: usize, preview_capacity: usize) -> Self {
        Self {
            number: 1,
            start: 0,
            bytes: 0,
            matcher: 0,
            before: VecDeque::with_capacity(before_capacity),
            before_capacity,
            preview: Vec::with_capacity(preview_capacity),
            preview_start: 0,
            match_offset: None,
        }
    }

    fn push(
        &mut self,
        byte: u8,
        offset: u64,
        pattern: &[u8],
        failure: &[usize],
        case_sensitive: bool,
        max_preview: usize,
    ) {
        self.bytes += 1;
        if self.match_offset.is_some() {
            if self.preview.len() < max_preview {
                self.preview.push(byte);
            }
            return;
        }

        if self.before.len() == self.before_capacity {
            self.before.pop_front();
        }
        self.before.push_back(byte);

        let candidate = fold_ascii(byte, case_sensitive);
        while self.matcher > 0 && pattern[self.matcher] != candidate {
            self.matcher = failure[self.matcher - 1];
        }
        if pattern[self.matcher] == candidate {
            self.matcher += 1;
        }
        if self.matcher == pattern.len() {
            let pattern_len = u64::try_from(pattern.len()).expect("pattern length fits u64");
            let window_len = u64::try_from(self.before.len()).expect("window length fits u64");
            self.match_offset = Some(offset + 1 - pattern_len);
            self.preview_start = self.bytes - window_len;
            self.preview.extend(self.before.iter().copied());
            self.matcher = failure[self.matcher - 1];
        }
    }

    fn take_match(&mut self) -> Option<ScanMatch> {
        let match_byte_offset = self.match_offset?;
        let preview_end = self.preview_start
            + u64::try_from(self.preview.len()).expect("preview length fits u64");
        let covers_end = preview_end == self.bytes;
        if covers_end && self.preview.last() == Some(&b'\r') {
            self.preview.pop();
        }
        let (content, content_lossy) = match String::from_utf8(self.preview.clone()) {
            Ok(content) => (content, false),
            Err(error) => (String::from_utf8_lossy(error.as_bytes()).into_owned(), true),
        };
        Some(ScanMatch {
            position: ScanMatchPosition {
                line_number: self.number,
                line_start_offset: self.start,
                match_byte_offset,
                original_line_bytes: self.bytes,
            },
            content,
            content_truncated: self.preview_start > 0 || !covers_end,
            content_lossy,
        })
    }

    fn reset(&mut self, number: u64, start: u64) {
        self.number = number;
        self.start = start;
        self.bytes = 0;
        self.matcher = 0;
        self.before.clear();
        self.preview.clear();
        self.preview_start = 0;
        self.match_offset = None;
    }
}

pub fn scan_reader<R: Read>(
    reader: &mut R,
    request: &ScanRequest,
) -> Result<ScanOutcome, ScanError> {
    request.validate()?;
    let pattern: Vec<u8> = request
        .keyword
        .bytes()
        .map(|byte| fold_ascii(byte, request.case_sensitive))
        .collect();
    let failure = failure_table(&pattern);
    let before_capacity = (request.limits.max_line_bytes - pattern.len()) / 2 + pattern.len();
    let mut line = LineState::new(before_capacity, request.limits.max_line_bytes);
    let mut buffer = vec![0_u8; request.limits.read_buffer_bytes];
    let mut outcome = ScanOutcome {
        results: Vec::with_capacity(request.limits.max_results),
        bytes_scanned: 0,
        lines_scanned: 0,
        returned_content_bytes: 0,
        stop_reason: ScanStopReason::Complete,
    };
    let mut bytes_since_check = 0_u64;

    loop {
        if let Some(reason) = stop_reason(request) {
            outcome.stop_reason = reason;
            return Ok(outcome);
        }
        if outcome.bytes_scanned >= request.limits.max_scan_bytes {
            outcome.stop_reason = ScanStopReason::ScanByteLimit;
            return Ok(outcome);
        }

        let remaining = request.limits.max_scan_bytes - outcome.bytes_scanned;
        let read_size = buffer
            .len()
            .min(usize::try_from(remaining).unwrap_or(usize::MAX));
        let count = match reader.read(&mut buffer[..read_size]) {
            Ok(0) => {
                if line.bytes > 0 {
                    outcome.lines_scanned += 1;
                    if let Some(reason) = finish_line(&mut line, &mut outcome, request.limits) {
                        outcome.stop_reason = reason;
                    }
                }
                return Ok(outcome);
            }
            Ok(count) => count,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(ScanError::Io(error)),
        };

        for &byte in &buffer[..count] {
            if bytes_since_check >= CHECK_INTERVAL_BYTES {
                if let Some(reason) = stop_reason(request) {
                    outcome.stop_reason = reason;
                    return Ok(outcome);
                }
                bytes_since_check = 0;
            }
            let offset = outcome.bytes_scanned;
            outcome.bytes_scanned += 1;
            bytes_since_check += 1;

            if byte == b'\n' {
                outcome.lines_scanned += 1;
                if let Some(reason) = finish_line(&mut line, &mut outcome, request.limits) {
                    outcome.stop_reason = reason;
                    return Ok(outcome);
                }
                line.reset(outcome.lines_scanned + 1, outcome.bytes_scanned);
            } else {
                line.push(
                    byte,
                    offset,
                    &pattern,
                    &failure,
                    request.case_sensitive,
                    request.limits.max_line_bytes,
                );
            }
        }
    }
}

fn finish_line(
    line: &mut LineState,
    outcome: &mut ScanOutcome,
    limits: ScanLimits,
) -> Option<ScanStopReason> {
    let found = line.take_match()?;
    let bytes = found.content.len();
    if outcome.returned_content_bytes.saturating_add(bytes) > limits.max_returned_content_bytes {
        return Some(ScanStopReason::ReturnedContentByteLimit);
    }
    outcome.returned_content_bytes += bytes;
    outcome.results.push(found);
    if outcome.results.len() >= limits.max_results {
        Some(ScanStopReason::ResultLimit)
    } else if outcome.returned_content_bytes >= limits.max_returned_content_bytes {
        Some(ScanStopReason::ReturnedContentByteLimit)
    } else {
        None
    }
}

fn stop_reason(request: &ScanRequest) -> Option<ScanStopReason> {
    if request.cancellation.is_cancelled() {
        Some(ScanStopReason::Cancelled)
    } else if request
        .deadline
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        Some(ScanStopReason::DeadlineExceeded)
    } else {
        None
    }
}

fn fold_ascii(byte: u8, case_sensitive: bool) -> u8 {
    if case_sensitive {
        byte
    } else {
        byte.to_ascii_lowercase()
    }
}

fn failure_table(pattern: &[u8]) -> Vec<usize> {
    let mut table = vec![0; pattern.len()];
    let mut prefix = 0;
    for index in 1..pattern.len() {
        while prefix > 0 && pattern[index] != pattern[prefix] {
            prefix = table[prefix - 1];
        }
        if pattern[index] == pattern[prefix] {
            prefix += 1;
            table[index] = prefix;
        }
    }
    table
}
