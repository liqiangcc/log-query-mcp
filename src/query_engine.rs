use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    fs::File,
    io::{ErrorKind, Read, Seek, SeekFrom},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset, Utc};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ConfiguredSource, MAX_RETURNED_CONTENT_BYTES, MAX_SCAN_RESULTS, ScanExecutor, ScanLimits,
    ScanMatch, ScanPosition, ScanRequest, ScanStopReason, ScanTaskError, SourceFileSnapshot,
    SourceRegistry, SourceRegistryError, TimeFilterDecision, TimeFilterError, TimeRange,
    TimestampObservation, TimestampParser,
};

const DEFAULT_READ_BUFFER_BYTES: usize = 64 * 1024;
const LOSSY_UTF8_EXPANSION_FACTOR: usize = 3;

#[derive(Debug, Clone)]
pub struct QueryRequest {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub max_results: Option<usize>,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl QueryRequest {
    #[must_use]
    pub fn new(source_ids: Vec<String>, keyword: impl Into<String>) -> Self {
        Self {
            source_ids,
            keyword: keyword.into(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            max_results: None,
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
    pub fn with_time_range(
        mut self,
        start_time: Option<String>,
        end_time: Option<String>,
    ) -> Self {
        self.start_time = start_time;
        self.end_time = end_time;
        self
    }

    #[must_use]
    pub fn with_max_results(mut self, max_results: usize) -> Self {
        self.max_results = Some(max_results);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryMatch {
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub line_number: u64,
    pub timestamp: Option<DateTime<FixedOffset>>,
    pub content: String,
    pub content_truncated: bool,
    pub content_lossy: bool,
    pub original_line_bytes: u64,
    pub line_start_offset: u64,
    pub match_byte_offset: u64,
}

impl QueryMatch {
    #[must_use]
    pub fn timestamp_rfc3339(&self) -> Option<String> {
        self.timestamp.as_ref().map(DateTime::to_rfc3339)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryPageStopReason {
    Complete,
    ResultLimit,
    ReturnedContentByteLimit,
    ScanByteLimit,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QuerySummary {
    pub files_considered: usize,
    pub files_scanned: usize,
    pub bytes_scanned: u64,
    pub lines_scanned: u64,
    pub raw_matches: u64,
    pub filtered_out_matches: u64,
    pub eligible_matches: u64,
    pub unknown_timestamp_matches: u64,
    pub malformed_timestamp_matches: u64,
    pub returned_results: usize,
    pub returned_content_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryPage {
    pub results: Vec<QueryMatch>,
    pub truncated: bool,
    pub stop_reason: QueryPageStopReason,
    pub summary: QuerySummary,
}

#[derive(Debug)]
pub struct QueryEngine {
    registry: Arc<SourceRegistry>,
    executor: ScanExecutor,
}

impl QueryEngine {
    pub fn new(registry: Arc<SourceRegistry>) -> Result<Self, QueryError> {
        let executor = ScanExecutor::new(registry.limits().max_concurrent_scans)?;
        Ok(Self { registry, executor })
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<SourceRegistry> {
        &self.registry
    }

    #[must_use]
    pub const fn executor(&self) -> &ScanExecutor {
        &self.executor
    }

    pub async fn execute(&self, request: QueryRequest) -> Result<QueryPage, QueryError> {
        let limits = self.registry.limits();
        let max_results = validate_request(&request, limits)?;
        let time_range = TimeRange::from_rfc3339(
            request.start_time.as_deref(),
            request.end_time.as_deref(),
        )?;
        let deadline = effective_deadline(request.deadline, limits.query_timeout_millis)?;
        check_interrupted(&request.cancellation, deadline)?;

        let selected_sources = self.registry.selected(&request.source_ids)?;
        let candidates = build_candidates(
            &selected_sources,
            limits.max_scan_files_per_query,
        )?;

        let mut summary = QuerySummary {
            files_considered: candidates.len(),
            ..QuerySummary::default()
        };
        let mut earliest = BinaryHeap::<RankedMatch>::with_capacity(max_results + 1);
        let mut page_scan_limited = false;

        'candidate: for candidate in &candidates {
            check_interrupted(&request.cancellation, deadline)?;
            summary.files_scanned += 1;
            let mut position = ScanPosition::default();

            while position.byte_offset < candidate.snapshot.size_at_snapshot() {
                check_interrupted(&request.cancellation, deadline)?;
                let remaining_page_bytes = limits
                    .max_scan_bytes_per_page
                    .saturating_sub(summary.bytes_scanned);
                if remaining_page_bytes == 0 {
                    page_scan_limited = true;
                    break 'candidate;
                }

                let snapshot_remaining = candidate
                    .snapshot
                    .size_at_snapshot()
                    .checked_sub(position.byte_offset)
                    .ok_or(QueryError::InvalidScanPosition)?;
                let bytes_to_read = snapshot_remaining.min(remaining_page_bytes);
                let fully_reads_snapshot = bytes_to_read == snapshot_remaining;
                let scan_budget = if fully_reads_snapshot {
                    bytes_to_read
                        .checked_add(1)
                        .unwrap_or(bytes_to_read)
                        .min(crate::MAX_SCAN_BYTES)
                } else {
                    bytes_to_read
                };

                let safe_file = candidate.source.open_snapshot_file(&candidate.snapshot)?;
                let mut file = safe_file.into_file();
                seek_to_scan_position(
                    &mut file,
                    position,
                    candidate.snapshot.size_at_snapshot(),
                )?;

                let chunk_result_limit = chunk_result_limit(limits.max_line_bytes);
                let scan_limits = ScanLimits {
                    max_scan_bytes: scan_budget,
                    max_results: chunk_result_limit,
                    max_line_bytes: limits.max_line_bytes,
                    max_returned_content_bytes: MAX_RETURNED_CONTENT_BYTES,
                    read_buffer_bytes: DEFAULT_READ_BUFFER_BYTES,
                };
                let scan_request = ScanRequest::new(request.keyword.clone())
                    .with_case_sensitive(request.case_sensitive)
                    .with_limits(scan_limits)
                    .with_start_position(position)
                    .with_deadline(deadline)
                    .with_cancellation(request.cancellation.clone());
                let outcome = self
                    .executor
                    .scan(file.take(bytes_to_read), scan_request)
                    .await?;

                summary.bytes_scanned = summary
                    .bytes_scanned
                    .checked_add(outcome.bytes_scanned)
                    .ok_or(QueryError::ResourceCounterOverflow)?;
                summary.lines_scanned = summary
                    .lines_scanned
                    .checked_add(outcome.lines_scanned)
                    .ok_or(QueryError::ResourceCounterOverflow)?;
                summary.raw_matches = summary
                    .raw_matches
                    .checked_add(
                        u64::try_from(outcome.results.len())
                            .map_err(|_| QueryError::ResourceCounterOverflow)?,
                    )
                    .ok_or(QueryError::ResourceCounterOverflow)?;

                process_matches(
                    candidate,
                    outcome.results,
                    &time_range,
                    max_results,
                    &mut earliest,
                    &mut summary,
                )?;

                let reached_snapshot_end = position
                    .byte_offset
                    .checked_add(outcome.bytes_scanned)
                    .is_some_and(|offset| offset >= candidate.snapshot.size_at_snapshot());

                match outcome.stop_reason {
                    ScanStopReason::Complete => break,
                    ScanStopReason::Cancelled => return Err(QueryError::Cancelled),
                    ScanStopReason::DeadlineExceeded => {
                        return Err(QueryError::DeadlineExceeded);
                    }
                    ScanStopReason::ResultLimit
                    | ScanStopReason::ReturnedContentByteLimit => {
                        if reached_snapshot_end {
                            break;
                        }
                        position = validated_next_position(position, outcome.next_position)?;
                    }
                    ScanStopReason::ScanByteLimit => {
                        if reached_snapshot_end {
                            break;
                        }
                        if summary.bytes_scanned >= limits.max_scan_bytes_per_page {
                            page_scan_limited = true;
                            break 'candidate;
                        }
                        position = validated_next_position(position, outcome.next_position)?;
                    }
                }
            }
        }

        finish_page(
            earliest,
            max_results,
            limits.max_returned_content_bytes,
            page_scan_limited,
            summary,
        )
    }
}

#[derive(Debug)]
struct FileCandidate {
    source: Arc<ConfiguredSource>,
    snapshot: SourceFileSnapshot,
    source_index: usize,
    file_index: usize,
    timestamp_parser: Option<TimestampParser>,
}

fn build_candidates(
    sources: &[Arc<ConfiguredSource>],
    max_files: usize,
) -> Result<Vec<FileCandidate>, QueryError> {
    let mut candidates = Vec::new();
    let mut remaining = max_files;

    for (source_index, source) in sources.iter().enumerate() {
        if remaining == 0 {
            return Err(QueryError::FileLimitExceeded);
        }
        let snapshots = source.snapshot_files(remaining)?;
        let timestamp_parser = source
            .timestamp_rule()
            .map(TimestampParser::new)
            .transpose()?;
        remaining = remaining
            .checked_sub(snapshots.len())
            .ok_or(QueryError::ResourceCounterOverflow)?;

        candidates.extend(
            snapshots
                .into_iter()
                .enumerate()
                .map(|(file_index, snapshot)| FileCandidate {
                    source: Arc::clone(source),
                    snapshot,
                    source_index,
                    file_index,
                    timestamp_parser: timestamp_parser.clone(),
                }),
        );
    }

    Ok(candidates)
}

fn process_matches(
    candidate: &FileCandidate,
    matches: Vec<ScanMatch>,
    time_range: &TimeRange,
    max_results: usize,
    earliest: &mut BinaryHeap<RankedMatch>,
    summary: &mut QuerySummary,
) -> Result<(), QueryError> {
    if matches.is_empty() {
        return Ok(());
    }

    let mut prefix_file = if candidate.timestamp_parser.is_some() {
        Some(
            candidate
                .source
                .open_snapshot_file(&candidate.snapshot)?
                .into_file(),
        )
    } else {
        None
    };

    for scan_match in matches {
        let observation = if let (Some(parser), Some(file)) =
            (candidate.timestamp_parser.as_ref(), prefix_file.as_mut())
        {
            let prefix = read_line_prefix(
                file,
                scan_match.line_start_offset,
                candidate.snapshot.size_at_snapshot(),
                parser.prefix_bytes(),
            )?;
            parser.observe(&prefix)
        } else {
            TimestampObservation {
                timestamp: None,
                malformed: false,
            }
        };

        match time_range.classify(&observation) {
            TimeFilterDecision::OutOfRange => {
                summary.filtered_out_matches = summary
                    .filtered_out_matches
                    .checked_add(1)
                    .ok_or(QueryError::ResourceCounterOverflow)?;
                continue;
            }
            TimeFilterDecision::UnknownTimestamp => {
                summary.unknown_timestamp_matches = summary
                    .unknown_timestamp_matches
                    .checked_add(1)
                    .ok_or(QueryError::ResourceCounterOverflow)?;
            }
            TimeFilterDecision::MalformedTimestamp => {
                summary.malformed_timestamp_matches = summary
                    .malformed_timestamp_matches
                    .checked_add(1)
                    .ok_or(QueryError::ResourceCounterOverflow)?;
            }
            TimeFilterDecision::InRange => {}
        }

        summary.eligible_matches = summary
            .eligible_matches
            .checked_add(1)
            .ok_or(QueryError::ResourceCounterOverflow)?;
        let timestamp_utc = observation
            .timestamp
            .as_ref()
            .map(|timestamp| timestamp.with_timezone(&Utc));
        let key = ResultSortKey {
            timestamp: timestamp_utc,
            source_index: candidate.source_index,
            file_index: candidate.file_index,
            line_number: scan_match.line_number,
            match_byte_offset: scan_match.match_byte_offset,
        };
        earliest.push(RankedMatch {
            key,
            value: QueryMatch {
                source_id: candidate.snapshot.source_id().to_owned(),
                file_id: candidate.snapshot.file_id().to_owned(),
                file_name: candidate.snapshot.display_name(),
                line_number: scan_match.line_number,
                timestamp: observation.timestamp,
                content: scan_match.content,
                content_truncated: scan_match.content_truncated,
                content_lossy: scan_match.content_lossy,
                original_line_bytes: scan_match.original_line_bytes,
                line_start_offset: scan_match.line_start_offset,
                match_byte_offset: scan_match.match_byte_offset,
            },
        });
        if earliest.len() > max_results {
            earliest.pop();
        }
    }
    Ok(())
}

fn finish_page(
    earliest: BinaryHeap<RankedMatch>,
    max_results: usize,
    max_content_bytes: usize,
    page_scan_limited: bool,
    mut summary: QuerySummary,
) -> Result<QueryPage, QueryError> {
    let mut ranked = earliest.into_vec();
    ranked.sort_by(|left, right| left.key.cmp(&right.key));

    let result_limited = summary.eligible_matches
        > u64::try_from(max_results).map_err(|_| QueryError::ResourceCounterOverflow)?;
    let mut content_limited = false;
    let mut returned_content_bytes = 0_usize;
    let mut results = Vec::with_capacity(ranked.len());

    for mut ranked_match in ranked {
        let remaining = max_content_bytes.saturating_sub(returned_content_bytes);
        if remaining == 0 {
            content_limited = true;
            break;
        }
        if ranked_match.value.content.len() > remaining {
            truncate_utf8(&mut ranked_match.value.content, remaining);
            ranked_match.value.content_truncated = true;
            content_limited = true;
        }
        returned_content_bytes = returned_content_bytes
            .checked_add(ranked_match.value.content.len())
            .ok_or(QueryError::ResourceCounterOverflow)?;
        results.push(ranked_match.value);
        if content_limited {
            break;
        }
    }

    summary.returned_results = results.len();
    summary.returned_content_bytes = returned_content_bytes;
    let stop_reason = if page_scan_limited {
        QueryPageStopReason::ScanByteLimit
    } else if content_limited {
        QueryPageStopReason::ReturnedContentByteLimit
    } else if result_limited {
        QueryPageStopReason::ResultLimit
    } else {
        QueryPageStopReason::Complete
    };

    Ok(QueryPage {
        results,
        truncated: stop_reason != QueryPageStopReason::Complete,
        stop_reason,
        summary,
    })
}

fn validate_request(
    request: &QueryRequest,
    limits: &crate::LimitsConfig,
) -> Result<usize, QueryError> {
    if request.source_ids.is_empty() || request.source_ids.len() > limits.max_sources_per_query {
        return Err(QueryError::InvalidArgument(
            "source_ids count is outside the service limit",
        ));
    }
    let mut unique_sources = HashSet::with_capacity(request.source_ids.len());
    if request
        .source_ids
        .iter()
        .any(|source_id| !unique_sources.insert(source_id))
    {
        return Err(QueryError::InvalidArgument(
            "source_ids must not contain duplicates",
        ));
    }
    let keyword_chars = request.keyword.chars().count();
    if keyword_chars == 0
        || keyword_chars > crate::MAX_SCAN_KEYWORD_CHARS
        || request.keyword.as_bytes().contains(&b'\n')
        || request.keyword.as_bytes().contains(&b'\r')
    {
        return Err(QueryError::InvalidArgument(
            "keyword is outside the v1 literal search contract",
        ));
    }

    let max_results = request
        .max_results
        .unwrap_or(limits.default_results_per_page);
    if max_results == 0 || max_results > limits.max_results_per_page {
        return Err(QueryError::InvalidArgument(
            "max_results is outside the service limit",
        ));
    }
    Ok(max_results)
}

fn effective_deadline(
    requested: Option<Instant>,
    timeout_millis: u64,
) -> Result<Instant, QueryError> {
    let server_deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout_millis))
        .ok_or(QueryError::DeadlineOverflow)?;
    Ok(requested.map_or(server_deadline, |deadline| deadline.min(server_deadline)))
}

fn check_interrupted(
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<(), QueryError> {
    if cancellation.is_cancelled() {
        return Err(QueryError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(QueryError::DeadlineExceeded);
    }
    Ok(())
}

fn seek_to_scan_position(
    file: &mut File,
    position: ScanPosition,
    snapshot_size: u64,
) -> Result<(), QueryError> {
    if position.line_number == 0 || position.byte_offset > snapshot_size {
        return Err(QueryError::InvalidScanPosition);
    }
    if position.byte_offset > 0 {
        file.seek(SeekFrom::Start(position.byte_offset - 1))?;
        let mut previous = [0_u8; 1];
        file.read_exact(&mut previous)
            .map_err(|error| map_position_read_error(error))?;
        if previous[0] != b'\n' {
            return Err(QueryError::ScanPositionNotLineBoundary);
        }
    }
    file.seek(SeekFrom::Start(position.byte_offset))?;
    Ok(())
}

fn validated_next_position(
    previous: ScanPosition,
    next: Option<ScanPosition>,
) -> Result<ScanPosition, QueryError> {
    let next = next.ok_or(QueryError::UnsafeContinuation)?;
    if next.byte_offset <= previous.byte_offset || next.line_number < previous.line_number {
        return Err(QueryError::UnsafeContinuation);
    }
    Ok(next)
}

fn read_line_prefix(
    file: &mut File,
    line_start_offset: u64,
    snapshot_size: u64,
    maximum_bytes: usize,
) -> Result<Vec<u8>, QueryError> {
    if line_start_offset >= snapshot_size {
        return Err(QueryError::InvalidScanPosition);
    }
    file.seek(SeekFrom::Start(line_start_offset))?;
    let available = snapshot_size - line_start_offset;
    let limit = available.min(
        u64::try_from(maximum_bytes).map_err(|_| QueryError::ResourceCounterOverflow)?,
    );
    let capacity = usize::try_from(limit).map_err(|_| QueryError::ResourceCounterOverflow)?;
    let mut prefix = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 64];

    while prefix.len() < capacity {
        let remaining = capacity - prefix.len();
        let read_len = remaining.min(buffer.len());
        let count = match file.read(&mut buffer[..read_len]) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(QueryError::Io(error)),
        };
        if let Some(newline) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            prefix.extend_from_slice(&buffer[..newline]);
            break;
        }
        prefix.extend_from_slice(&buffer[..count]);
    }
    Ok(prefix)
}

fn map_position_read_error(error: std::io::Error) -> QueryError {
    if error.kind() == ErrorKind::UnexpectedEof {
        QueryError::InvalidScanPosition
    } else {
        QueryError::Io(error)
    }
}

fn chunk_result_limit(max_line_bytes: usize) -> usize {
    let expanded_line = max_line_bytes
        .saturating_mul(LOSSY_UTF8_EXPANSION_FACTOR)
        .max(1);
    (MAX_RETURNED_CONTENT_BYTES / expanded_line)
        .clamp(1, MAX_SCAN_RESULTS)
}

fn truncate_utf8(content: &mut String, maximum_bytes: usize) {
    if content.len() <= maximum_bytes {
        return;
    }
    let mut boundary = maximum_bytes;
    while boundary > 0 && !content.is_char_boundary(boundary) {
        boundary -= 1;
    }
    content.truncate(boundary);
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResultSortKey {
    timestamp: Option<DateTime<Utc>>,
    source_index: usize,
    file_index: usize,
    line_number: u64,
    match_byte_offset: u64,
}

impl Ord for ResultSortKey {
    fn cmp(&self, other: &Self) -> Ordering {
        timestamp_cmp(&self.timestamp, &other.timestamp)
            .then_with(|| self.source_index.cmp(&other.source_index))
            .then_with(|| self.file_index.cmp(&other.file_index))
            .then_with(|| self.line_number.cmp(&other.line_number))
            .then_with(|| self.match_byte_offset.cmp(&other.match_byte_offset))
    }
}

impl PartialOrd for ResultSortKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn timestamp_cmp(left: &Option<DateTime<Utc>>, right: &Option<DateTime<Utc>>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

#[derive(Debug)]
struct RankedMatch {
    key: ResultSortKey,
    value: QueryMatch,
}

impl PartialEq for RankedMatch {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RankedMatch {}

impl Ord for RankedMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for RankedMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("invalid query: {0}")]
    InvalidArgument(&'static str),

    #[error("query deadline cannot be represented")]
    DeadlineOverflow,

    #[error("query was cancelled")]
    Cancelled,

    #[error("query deadline was exceeded")]
    DeadlineExceeded,

    #[error("query candidate file limit was exceeded")]
    FileLimitExceeded,

    #[error("scan position is invalid for the file snapshot")]
    InvalidScanPosition,

    #[error("scan position is not aligned to a log line boundary")]
    ScanPositionNotLineBoundary,

    #[error("scanner stopped without a safe continuation position")]
    UnsafeContinuation,

    #[error("query resource counter overflowed")]
    ResourceCounterOverflow,

    #[error(transparent)]
    TimeFilter(#[from] TimeFilterError),

    #[error(transparent)]
    SourceRegistry(#[from] SourceRegistryError),

    #[error(transparent)]
    ScanTask(#[from] ScanTaskError),

    #[error("query file operation failed")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::{AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, TimestampRule};

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sorts_multi_source_results_by_timestamp_with_unknowns_last() {
        let first_root = tempdir().expect("first root should be created");
        let second_root = tempdir().expect("second root should be created");
        fs::write(
            first_root.path().join("application.log"),
            concat!(
                "2026-06-19T14:02:00+09:00 MATCH later\n",
                "continuation MATCH unknown\n"
            ),
        )
        .expect("first fixture should be written");
        fs::write(
            second_root.path().join("application.log"),
            "2026-06-19T14:01:00+09:00 MATCH earlier\n",
        )
        .expect("second fixture should be written");
        let engine = engine(
            vec![
                source(first_root.path(), "first"),
                source(second_root.path(), "second"),
            ],
            LimitsConfig::default(),
        );

        let page = engine
            .execute(QueryRequest::new(
                vec!["first".to_owned(), "second".to_owned()],
                "MATCH",
            ))
            .await
            .expect("query should succeed");

        assert_eq!(
            page.results
                .iter()
                .map(|result| result.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "2026-06-19T14:01:00+09:00 MATCH earlier",
                "2026-06-19T14:02:00+09:00 MATCH later",
                "continuation MATCH unknown"
            ]
        );
        assert_eq!(page.summary.unknown_timestamp_matches, 1);
        assert_eq!(page.stop_reason, QueryPageStopReason::Complete);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn applies_start_inclusive_end_exclusive_and_keeps_unknown_times() {
        let root = tempdir().expect("root should be created");
        fs::write(
            root.path().join("application.log"),
            concat!(
                "2026-06-19T14:00:00+09:00 MATCH start\n",
                "2026-06-19T14:59:59+09:00 MATCH inside\n",
                "2026-06-19T15:00:00+09:00 MATCH end\n",
                "2026-99-99T99:99:99Z MATCH malformed\n",
                "stack MATCH unknown\n"
            ),
        )
        .expect("fixture should be written");
        let engine = engine(vec![source(root.path(), "payment")], LimitsConfig::default());
        let request = QueryRequest::new(vec!["payment".to_owned()], "MATCH").with_time_range(
            Some("2026-06-19T14:00:00+09:00".to_owned()),
            Some("2026-06-19T15:00:00+09:00".to_owned()),
        );

        let page = engine.execute(request).await.expect("query should succeed");

        assert_eq!(page.results.len(), 4);
        assert!(page.results.iter().all(|result| !result.content.ends_with(" end")));
        assert_eq!(page.summary.filtered_out_matches, 1);
        assert_eq!(page.summary.malformed_timestamp_matches, 1);
        assert_eq!(page.summary.unknown_timestamp_matches, 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_globally_earliest_result_when_page_size_is_one() {
        let later_root = tempdir().expect("later root should be created");
        let earlier_root = tempdir().expect("earlier root should be created");
        fs::write(
            later_root.path().join("application.log"),
            "2026-06-19T14:10:00+09:00 MATCH later\n",
        )
        .expect("later fixture should be written");
        fs::write(
            earlier_root.path().join("application.log"),
            "2026-06-19T14:00:00+09:00 MATCH earlier\n",
        )
        .expect("earlier fixture should be written");
        let engine = engine(
            vec![
                source(later_root.path(), "later"),
                source(earlier_root.path(), "earlier"),
            ],
            LimitsConfig::default(),
        );

        let page = engine
            .execute(
                QueryRequest::new(vec!["later".to_owned(), "earlier".to_owned()], "MATCH")
                    .with_max_results(1),
            )
            .await
            .expect("query should succeed");

        assert_eq!(page.results.len(), 1);
        assert!(page.results[0].content.ends_with("earlier"));
        assert!(page.truncated);
        assert_eq!(page.stop_reason, QueryPageStopReason::ResultLimit);
        assert_eq!(page.summary.eligible_matches, 2);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn aggregates_page_scan_byte_limit() {
        let root = tempdir().expect("root should be created");
        fs::write(root.path().join("application.log"), "abcdefghij MATCH later\n")
            .expect("fixture should be written");
        let mut limits = LimitsConfig::default();
        limits.max_scan_bytes_per_page = 10;
        let engine = engine(vec![source(root.path(), "payment")], limits);

        let page = engine
            .execute(QueryRequest::new(vec!["payment".to_owned()], "MATCH"))
            .await
            .expect("query should return a bounded page");

        assert!(page.results.is_empty());
        assert!(page.truncated);
        assert_eq!(page.stop_reason, QueryPageStopReason::ScanByteLimit);
        assert_eq!(page.summary.bytes_scanned, 10);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_duplicate_sources_and_pre_cancelled_query() {
        let root = tempdir().expect("root should be created");
        fs::write(root.path().join("application.log"), "MATCH\n")
            .expect("fixture should be written");
        let engine = engine(vec![source(root.path(), "payment")], LimitsConfig::default());

        assert!(matches!(
            engine
                .execute(QueryRequest::new(
                    vec!["payment".to_owned(), "payment".to_owned()],
                    "MATCH"
                ))
                .await,
            Err(QueryError::InvalidArgument(_))
        ));

        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert!(matches!(
            engine
                .execute(
                    QueryRequest::new(vec!["payment".to_owned()], "MATCH")
                        .with_cancellation(cancellation)
                )
                .await,
            Err(QueryError::Cancelled)
        ));
    }

    fn engine(sources: Vec<LogSourceConfig>, limits: LimitsConfig) -> QueryEngine {
        let registry = SourceRegistry::from_config(AppConfig {
            version: CONFIG_VERSION,
            sources,
            limits,
        })
        .expect("registry should build");
        QueryEngine::new(Arc::new(registry)).expect("engine should build")
    }

    fn source(root: &std::path::Path, source_id: &str) -> LogSourceConfig {
        LogSourceConfig {
            source_id: source_id.to_owned(),
            name: source_id.to_owned(),
            description: String::new(),
            service: source_id.to_owned(),
            environment: "test".to_owned(),
            tags: Vec::new(),
            enabled: true,
            encoding: Encoding::Utf8,
            root: root.to_path_buf(),
            files: vec![PathBuf::from("application.log")],
            directories: Vec::new(),
            timestamp_rule: Some(TimestampRule::Rfc3339 { prefix_bytes: 64 }),
        }
    }
}
