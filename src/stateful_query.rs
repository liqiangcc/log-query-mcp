use std::{
    cmp::Ordering,
    collections::{BinaryHeap, HashSet},
    io::{ErrorKind, Read, Seek, SeekFrom},
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{DateTime, FixedOffset};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ConfiguredSource, CumulativeQueryUsage, CursorCandidate, GenerationPin,
    MAX_RETURNED_CONTENT_BYTES, MAX_SCAN_RESULTS, MatchReferenceData, MatchReferenceStore,
    QueryBinding, QueryMatch, QueryPageStopReason, QueryStateError, QuerySummary, ResultWatermark,
    ScanExecutor, ScanLimits, ScanMatch, ScanPosition, ScanRequest, ScanStopReason, ScanTaskError,
    SearchCursorData, SearchCursorStore, SourceRegistry, SourceRegistryError, TimeFilterDecision,
    TimeFilterError, TimeRange, TimestampObservation, TimestampParser,
};

const DEFAULT_READ_BUFFER_BYTES: usize = 64 * 1024;
const LOSSY_UTF8_EXPANSION_FACTOR: usize = 3;

#[derive(Debug, Clone)]
pub struct StatefulQueryRequest {
    pub source_ids: Vec<String>,
    pub keyword: String,
    pub case_sensitive: bool,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub max_results: Option<usize>,
    pub cursor: Option<String>,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl StatefulQueryRequest {
    #[must_use]
    pub fn new(source_ids: Vec<String>, keyword: impl Into<String>) -> Self {
        Self {
            source_ids,
            keyword: keyword.into(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            max_results: None,
            cursor: None,
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
    pub fn with_time_range(mut self, start_time: Option<String>, end_time: Option<String>) -> Self {
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
    pub fn with_cursor(mut self, cursor: impl Into<String>) -> Self {
        self.cursor = Some(cursor.into());
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
pub struct RegisteredQueryMatch {
    pub match_ref: String,
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

impl RegisteredQueryMatch {
    #[must_use]
    pub fn timestamp_rfc3339(&self) -> Option<String> {
        self.timestamp.as_ref().map(DateTime::to_rfc3339)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatefulQuerySummary {
    pub page: QuerySummary,
    pub skipped_before_watermark: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatefulQueryPage {
    pub results: Vec<RegisteredQueryMatch>,
    pub truncated: bool,
    pub stop_reason: QueryPageStopReason,
    pub summary: StatefulQuerySummary,
    pub next_cursor: Option<String>,
    pub cumulative_usage: CumulativeQueryUsage,
    pub continuation_unavailable: bool,
}

#[derive(Debug)]
pub struct StatefulQueryService {
    registry: Arc<SourceRegistry>,
    executor: ScanExecutor,
    cursor_store: Arc<SearchCursorStore>,
    match_references: Arc<MatchReferenceStore>,
}

impl StatefulQueryService {
    pub fn new(registry: Arc<SourceRegistry>) -> Result<Self, StatefulQueryError> {
        let limits = registry.limits();
        let executor = ScanExecutor::new(limits.max_concurrent_scans)?;
        let cursor_store = Arc::new(SearchCursorStore::new(
            limits.cursor_capacity,
            Duration::from_secs(limits.cursor_ttl_seconds),
        )?);
        let match_references = Arc::new(MatchReferenceStore::new(
            limits.match_reference_capacity,
            Duration::from_secs(limits.match_reference_ttl_seconds),
        )?);
        Ok(Self {
            registry,
            executor,
            cursor_store,
            match_references,
        })
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<SourceRegistry> {
        &self.registry
    }

    #[must_use]
    pub fn cursor_store(&self) -> &Arc<SearchCursorStore> {
        &self.cursor_store
    }

    #[must_use]
    pub fn match_reference_store(&self) -> &Arc<MatchReferenceStore> {
        &self.match_references
    }

    pub async fn search(
        &self,
        request: StatefulQueryRequest,
    ) -> Result<StatefulQueryPage, StatefulQueryError> {
        let limits = self.registry.limits();
        let (binding, time_range) = query_binding(&request, limits)?;
        let deadline = effective_deadline(request.deadline, limits.query_timeout_millis)?;
        check_interrupted(&request.cancellation, deadline)?;

        let lease = request
            .cursor
            .as_deref()
            .map(|token| self.cursor_store.begin(token, &binding))
            .transpose()?;
        let (candidates, after, mut usage) = if let Some(cursor_lease) = lease.as_ref() {
            let data = cursor_lease.data();
            (
                data.candidates.clone(),
                Some(data.after.clone()),
                data.usage.clone(),
            )
        } else {
            (
                build_candidates(
                    &self.registry,
                    &binding.source_ids,
                    limits.max_scan_files_per_query,
                )
                .await?,
                None,
                CumulativeQueryUsage::default(),
            )
        };

        usage.validate()?;
        let scanned = self
            .scan_page(
                &binding,
                &time_range,
                &candidates,
                after.as_ref(),
                deadline,
                &request.cancellation,
            )
            .await?;

        let usage_result = usage.add_page(&scanned.summary.page);
        let cumulative_limited = match usage_result {
            Ok(()) => false,
            Err(QueryStateError::CumulativeLimit) => true,
            Err(error) => return Err(error.into()),
        };

        let mut registered = Vec::with_capacity(scanned.results.len());
        for result in &scanned.results {
            let match_ref = self.match_references.insert_with_pin(
                result.match_reference.clone(),
                result.generation_pin.clone(),
            )?;
            registered.push(RegisteredQueryMatch {
                match_ref,
                source_id: result.value.source_id.clone(),
                file_id: result.value.file_id.clone(),
                file_name: result.value.file_name.clone(),
                line_number: result.value.line_number,
                timestamp: result.value.timestamp,
                content: result.value.content.clone(),
                content_truncated: result.value.content_truncated,
                content_lossy: result.value.content_lossy,
                original_line_bytes: result.value.original_line_bytes,
                line_start_offset: result.value.line_start_offset,
                match_byte_offset: result.value.match_byte_offset,
            });
        }

        let needs_continuation = matches!(
            scanned.stop_reason,
            QueryPageStopReason::ResultLimit | QueryPageStopReason::ReturnedContentByteLimit
        );
        let continuation_state = if needs_continuation && !cumulative_limited {
            scanned.results.last().map(|last| SearchCursorData {
                query: binding.clone(),
                candidates: candidates.clone(),
                after: last.key.clone(),
                usage: usage.clone(),
            })
        } else {
            None
        };

        let next_cursor = match lease {
            Some(cursor_lease) => cursor_lease.commit(continuation_state)?,
            None => continuation_state
                .map(|state| self.cursor_store.insert(state))
                .transpose()?,
        };
        let continuation_unavailable = scanned.stop_reason == QueryPageStopReason::ScanByteLimit
            || (needs_continuation && next_cursor.is_none());

        Ok(StatefulQueryPage {
            results: registered,
            truncated: scanned.stop_reason != QueryPageStopReason::Complete,
            stop_reason: scanned.stop_reason,
            summary: scanned.summary,
            next_cursor,
            cumulative_usage: usage,
            continuation_unavailable,
        })
    }

    async fn scan_page(
        &self,
        binding: &QueryBinding,
        time_range: &TimeRange,
        candidates: &[CursorCandidate],
        after: Option<&ResultWatermark>,
        deadline: Instant,
        cancellation: &CancellationToken,
    ) -> Result<ScannedPage, StatefulQueryError> {
        let limits = self.registry.limits();
        if candidates.len() > limits.max_scan_files_per_query {
            return Err(StatefulQueryError::FileLimitExceeded);
        }

        let mut summary = StatefulQuerySummary {
            page: QuerySummary {
                files_considered: candidates.len(),
                ..QuerySummary::default()
            },
            skipped_before_watermark: 0,
        };
        let mut earliest = BinaryHeap::<RankedRegisteredMatch>::with_capacity(
            binding.max_results.saturating_add(1),
        );
        let mut page_scan_limited = false;

        'candidate: for candidate in candidates {
            check_interrupted(cancellation, deadline)?;
            validate_candidate_binding(binding, candidate)?;
            let source = self
                .registry
                .get(candidate.snapshot.source_id())
                .ok_or_else(|| {
                    SourceRegistryError::UnknownSource(candidate.snapshot.source_id().to_owned())
                })?;
            let timestamp_parser = source
                .timestamp_rule()
                .map(TimestampParser::new)
                .transpose()?;
            summary.page.files_scanned = summary.page.files_scanned.saturating_add(1);
            let mut position = ScanPosition::default();

            while position.byte_offset < candidate.snapshot.size_at_snapshot() {
                check_interrupted(cancellation, deadline)?;
                let remaining_page_bytes = limits
                    .max_scan_bytes_per_page
                    .saturating_sub(summary.page.bytes_scanned);
                if remaining_page_bytes == 0 {
                    page_scan_limited = true;
                    break 'candidate;
                }

                let snapshot_remaining = candidate
                    .snapshot
                    .size_at_snapshot()
                    .checked_sub(position.byte_offset)
                    .ok_or(StatefulQueryError::InvalidScanPosition)?;
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

                let safe_file = source.open_snapshot_file(&candidate.snapshot)?;
                let mut file = safe_file;
                seek_to_scan_position(&mut file, position, candidate.snapshot.size_at_snapshot())?;

                let scan_limits = ScanLimits {
                    max_scan_bytes: scan_budget,
                    max_results: chunk_result_limit(limits.max_line_bytes),
                    max_line_bytes: limits.max_line_bytes,
                    max_returned_content_bytes: MAX_RETURNED_CONTENT_BYTES,
                    read_buffer_bytes: DEFAULT_READ_BUFFER_BYTES,
                };
                let scan_request = ScanRequest::new(binding.keyword.clone())
                    .with_case_sensitive(binding.case_sensitive)
                    .with_limits(scan_limits)
                    .with_start_position(position)
                    .with_deadline(deadline)
                    .with_cancellation(cancellation.clone());
                let outcome = self
                    .executor
                    .scan(file.take(bytes_to_read), scan_request)
                    .await?;

                accumulate_scan_summary(&mut summary.page, &outcome)?;
                process_matches(
                    &source,
                    candidate,
                    outcome.results,
                    binding,
                    time_range,
                    timestamp_parser.as_ref(),
                    after,
                    &mut earliest,
                    &mut summary,
                )?;

                let reached_snapshot_end = position
                    .byte_offset
                    .checked_add(outcome.bytes_scanned)
                    .is_some_and(|offset| offset >= candidate.snapshot.size_at_snapshot());
                match outcome.stop_reason {
                    ScanStopReason::Complete => break,
                    ScanStopReason::Cancelled => return Err(StatefulQueryError::Cancelled),
                    ScanStopReason::DeadlineExceeded => {
                        return Err(StatefulQueryError::DeadlineExceeded);
                    }
                    ScanStopReason::ResultLimit | ScanStopReason::ReturnedContentByteLimit => {
                        if reached_snapshot_end {
                            break;
                        }
                        position = validated_next_position(position, outcome.next_position)?;
                    }
                    ScanStopReason::ScanByteLimit => {
                        if reached_snapshot_end {
                            break;
                        }
                        if summary.page.bytes_scanned >= limits.max_scan_bytes_per_page {
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
            binding.max_results,
            limits.max_returned_content_bytes,
            page_scan_limited,
            summary,
        )
    }
}

#[derive(Debug)]
struct RankedRegisteredMatch {
    key: ResultWatermark,
    value: QueryMatch,
    match_reference: MatchReferenceData,
    generation_pin: Option<GenerationPin>,
}

impl PartialEq for RankedRegisteredMatch {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for RankedRegisteredMatch {}

impl Ord for RankedRegisteredMatch {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

impl PartialOrd for RankedRegisteredMatch {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Debug)]
struct ScannedPage {
    results: Vec<RankedRegisteredMatch>,
    stop_reason: QueryPageStopReason,
    summary: StatefulQuerySummary,
}

fn query_binding(
    request: &StatefulQueryRequest,
    limits: &crate::LimitsConfig,
) -> Result<(QueryBinding, TimeRange), StatefulQueryError> {
    if request.source_ids.is_empty() || request.source_ids.len() > limits.max_sources_per_query {
        return Err(StatefulQueryError::InvalidArgument(
            "source_ids count is outside the service limit",
        ));
    }
    let mut unique_sources = HashSet::with_capacity(request.source_ids.len());
    if request
        .source_ids
        .iter()
        .any(|source_id| !unique_sources.insert(source_id))
    {
        return Err(StatefulQueryError::InvalidArgument(
            "source_ids must not contain duplicates",
        ));
    }
    let max_results = request
        .max_results
        .unwrap_or(limits.default_results_per_page);
    if max_results == 0 || max_results > limits.max_results_per_page {
        return Err(StatefulQueryError::InvalidArgument(
            "max_results is outside the service limit",
        ));
    }
    let time_range =
        TimeRange::from_rfc3339(request.start_time.as_deref(), request.end_time.as_deref())?;
    let binding = QueryBinding {
        source_ids: request.source_ids.clone(),
        keyword: request.keyword.clone(),
        case_sensitive: request.case_sensitive,
        start_time: time_range.start,
        end_time: time_range.end,
        max_results,
    };
    binding.validate()?;
    Ok((binding, time_range))
}

async fn build_candidates(
    registry: &SourceRegistry,
    source_ids: &[String],
    max_files: usize,
) -> Result<Vec<CursorCandidate>, StatefulQueryError> {
    let sources = registry.selected(source_ids)?;
    let mut candidates = Vec::new();
    let mut remaining = max_files;

    for (source_index, source) in sources.iter().enumerate() {
        if remaining == 0 {
            return Err(StatefulQueryError::FileLimitExceeded);
        }
        let snapshots = source.query_snapshot_files(remaining).await?;
        if snapshots
            .iter()
            .any(|snapshot| !snapshot.has_complete_coverage())
        {
            return Err(StatefulQueryError::CacheScopeExceeded);
        }
        remaining = remaining
            .checked_sub(snapshots.len())
            .ok_or(StatefulQueryError::ResourceCounterOverflow)?;
        candidates.extend(
            snapshots
                .into_iter()
                .enumerate()
                .map(|(file_index, snapshot)| CursorCandidate {
                    source_index,
                    file_index,
                    snapshot,
                }),
        );
    }
    Ok(candidates)
}

fn validate_candidate_binding(
    binding: &QueryBinding,
    candidate: &CursorCandidate,
) -> Result<(), StatefulQueryError> {
    if candidate.source_index >= binding.source_ids.len()
        || candidate.snapshot.source_id() != binding.source_ids[candidate.source_index]
    {
        return Err(StatefulQueryError::InvalidCursorState);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn process_matches(
    source: &Arc<ConfiguredSource>,
    candidate: &CursorCandidate,
    matches: Vec<ScanMatch>,
    binding: &QueryBinding,
    time_range: &TimeRange,
    timestamp_parser: Option<&TimestampParser>,
    after: Option<&ResultWatermark>,
    earliest: &mut BinaryHeap<RankedRegisteredMatch>,
    summary: &mut StatefulQuerySummary,
) -> Result<(), StatefulQueryError> {
    if matches.is_empty() {
        return Ok(());
    }

    let mut prefix_file = if timestamp_parser.is_some() {
        Some(source.open_snapshot_file(&candidate.snapshot)?)
    } else {
        None
    };

    for scan_match in matches {
        let observation =
            if let (Some(parser), Some(file)) = (timestamp_parser, prefix_file.as_mut()) {
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
                summary.page.filtered_out_matches =
                    checked_add_u64(summary.page.filtered_out_matches, 1)?;
                continue;
            }
            TimeFilterDecision::UnknownTimestamp => {
                summary.page.unknown_timestamp_matches =
                    checked_add_u64(summary.page.unknown_timestamp_matches, 1)?;
            }
            TimeFilterDecision::MalformedTimestamp => {
                summary.page.malformed_timestamp_matches =
                    checked_add_u64(summary.page.malformed_timestamp_matches, 1)?;
            }
            TimeFilterDecision::InRange => {}
        }

        let key = ResultWatermark {
            timestamp_utc: observation.timestamp.as_ref().map(DateTime::to_utc),
            source_index: candidate.source_index,
            file_index: candidate.file_index,
            line_number: scan_match.line_number,
            match_byte_offset: scan_match.match_byte_offset,
        };
        if after.is_some_and(|watermark| key <= *watermark) {
            summary.skipped_before_watermark =
                checked_add_u64(summary.skipped_before_watermark, 1)?;
            continue;
        }

        summary.page.eligible_matches = checked_add_u64(summary.page.eligible_matches, 1)?;
        let match_reference = MatchReferenceData {
            source_id: candidate.snapshot.source_id().to_owned(),
            file_id: candidate.snapshot.file_id().to_owned(),
            relative_path: candidate.snapshot.relative_path().to_path_buf(),
            file_identity: candidate.snapshot.identity(),
            file_size_at_match: candidate.snapshot.size_at_snapshot(),
            line_number: scan_match.line_number,
            line_start_offset: scan_match.line_start_offset,
            match_byte_offset: scan_match.match_byte_offset,
            keyword: binding.keyword.clone(),
            case_sensitive: binding.case_sensitive,
        };
        match_reference.validate()?;
        earliest.push(RankedRegisteredMatch {
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
            match_reference,
            generation_pin: candidate.snapshot.generation_pin().cloned(),
        });
        if earliest.len() > binding.max_results {
            earliest.pop();
        }
    }
    Ok(())
}

fn finish_page(
    earliest: BinaryHeap<RankedRegisteredMatch>,
    max_results: usize,
    max_content_bytes: usize,
    page_scan_limited: bool,
    mut summary: StatefulQuerySummary,
) -> Result<ScannedPage, StatefulQueryError> {
    let mut ranked = earliest.into_vec();
    ranked.sort_by(|left, right| left.key.cmp(&right.key));

    let result_limited = summary.page.eligible_matches
        > u64::try_from(max_results).map_err(|_| StatefulQueryError::ResourceCounterOverflow)?;
    let mut content_limited = false;
    let mut returned_content_bytes = 0_usize;
    let mut results = Vec::with_capacity(ranked.len());

    for mut result in ranked {
        let remaining = max_content_bytes.saturating_sub(returned_content_bytes);
        if remaining == 0 {
            content_limited = true;
            break;
        }
        if result.value.content.len() > remaining {
            truncate_utf8(&mut result.value.content, remaining);
            result.value.content_truncated = true;
            content_limited = true;
        }
        returned_content_bytes = returned_content_bytes
            .checked_add(result.value.content.len())
            .ok_or(StatefulQueryError::ResourceCounterOverflow)?;
        results.push(result);
        if content_limited {
            break;
        }
    }

    summary.page.returned_results = results.len();
    summary.page.returned_content_bytes = returned_content_bytes;
    let stop_reason = if page_scan_limited {
        QueryPageStopReason::ScanByteLimit
    } else if content_limited {
        QueryPageStopReason::ReturnedContentByteLimit
    } else if result_limited {
        QueryPageStopReason::ResultLimit
    } else {
        QueryPageStopReason::Complete
    };
    Ok(ScannedPage {
        results,
        stop_reason,
        summary,
    })
}

fn accumulate_scan_summary(
    summary: &mut QuerySummary,
    outcome: &crate::ScanOutcome,
) -> Result<(), StatefulQueryError> {
    summary.bytes_scanned = checked_add_u64(summary.bytes_scanned, outcome.bytes_scanned)?;
    summary.lines_scanned = checked_add_u64(summary.lines_scanned, outcome.lines_scanned)?;
    summary.raw_matches = checked_add_u64(
        summary.raw_matches,
        u64::try_from(outcome.results.len())
            .map_err(|_| StatefulQueryError::ResourceCounterOverflow)?,
    )?;
    Ok(())
}

fn checked_add_u64(left: u64, right: u64) -> Result<u64, StatefulQueryError> {
    left.checked_add(right)
        .ok_or(StatefulQueryError::ResourceCounterOverflow)
}

fn effective_deadline(
    requested: Option<Instant>,
    timeout_millis: u64,
) -> Result<Instant, StatefulQueryError> {
    let server_deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout_millis))
        .ok_or(StatefulQueryError::DeadlineOverflow)?;
    Ok(requested.map_or(server_deadline, |deadline| deadline.min(server_deadline)))
}

fn check_interrupted(
    cancellation: &CancellationToken,
    deadline: Instant,
) -> Result<(), StatefulQueryError> {
    if cancellation.is_cancelled() {
        return Err(StatefulQueryError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(StatefulQueryError::DeadlineExceeded);
    }
    Ok(())
}

fn seek_to_scan_position<R: Read + Seek>(
    file: &mut R,
    position: ScanPosition,
    snapshot_size: u64,
) -> Result<(), StatefulQueryError> {
    if position.line_number == 0 || position.byte_offset > snapshot_size {
        return Err(StatefulQueryError::InvalidScanPosition);
    }
    if position.byte_offset > 0 {
        file.seek(SeekFrom::Start(position.byte_offset - 1))?;
        let mut previous = [0_u8; 1];
        file.read_exact(&mut previous)
            .map_err(map_position_read_error)?;
        if previous[0] != b'\n' {
            return Err(StatefulQueryError::ScanPositionNotLineBoundary);
        }
    }
    file.seek(SeekFrom::Start(position.byte_offset))?;
    Ok(())
}

fn validated_next_position(
    previous: ScanPosition,
    next: Option<ScanPosition>,
) -> Result<ScanPosition, StatefulQueryError> {
    let next = next.ok_or(StatefulQueryError::UnsafeContinuation)?;
    if next.byte_offset <= previous.byte_offset || next.line_number < previous.line_number {
        return Err(StatefulQueryError::UnsafeContinuation);
    }
    Ok(next)
}

fn read_line_prefix<R: Read + Seek>(
    file: &mut R,
    line_start_offset: u64,
    snapshot_size: u64,
    maximum_bytes: usize,
) -> Result<Vec<u8>, StatefulQueryError> {
    if line_start_offset >= snapshot_size {
        return Err(StatefulQueryError::InvalidScanPosition);
    }
    file.seek(SeekFrom::Start(line_start_offset))?;
    let available = snapshot_size - line_start_offset;
    let limit = available.min(
        u64::try_from(maximum_bytes).map_err(|_| StatefulQueryError::ResourceCounterOverflow)?,
    );
    let capacity =
        usize::try_from(limit).map_err(|_| StatefulQueryError::ResourceCounterOverflow)?;
    let mut prefix = Vec::with_capacity(capacity);
    let mut buffer = [0_u8; 64];

    while prefix.len() < capacity {
        let remaining = capacity - prefix.len();
        let read_len = remaining.min(buffer.len());
        let count = match file.read(&mut buffer[..read_len]) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(StatefulQueryError::Io(error)),
        };
        if let Some(newline) = buffer[..count].iter().position(|byte| *byte == b'\n') {
            prefix.extend_from_slice(&buffer[..newline]);
            break;
        }
        prefix.extend_from_slice(&buffer[..count]);
    }
    Ok(prefix)
}

fn map_position_read_error(error: std::io::Error) -> StatefulQueryError {
    if error.kind() == ErrorKind::UnexpectedEof {
        StatefulQueryError::InvalidScanPosition
    } else {
        StatefulQueryError::Io(error)
    }
}

fn chunk_result_limit(max_line_bytes: usize) -> usize {
    let expanded_line = max_line_bytes
        .saturating_mul(LOSSY_UTF8_EXPANSION_FACTOR)
        .max(1);
    (MAX_RETURNED_CONTENT_BYTES / expanded_line).clamp(1, MAX_SCAN_RESULTS)
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

#[derive(Debug, Error)]
pub enum StatefulQueryError {
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

    #[error("cursor candidate state is invalid")]
    InvalidCursorState,

    #[error("scan position is invalid for the file snapshot")]
    InvalidScanPosition,

    #[error("scan position is not aligned to a log line boundary")]
    ScanPositionNotLineBoundary,

    #[error("scanner stopped without a safe continuation position")]
    UnsafeContinuation,

    #[error("query cannot prove that the local cache covers the requested remote log scope")]
    CacheScopeExceeded,

    #[error("query resource counter overflowed")]
    ResourceCounterOverflow,

    #[error(transparent)]
    QueryState(#[from] QueryStateError),

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

    use crate::{
        AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, TimestampRule,
    };

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn paginates_globally_sorted_results_and_registers_match_references() {
        let root = tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            concat!(
                "2026-06-19T14:00:01+09:00 MATCH one\n",
                "2026-06-19T14:00:02+09:00 MATCH two\n",
                "2026-06-19T14:00:03+09:00 MATCH three\n",
                "2026-06-19T14:00:04+09:00 MATCH four\n",
                "2026-06-19T14:00:05+09:00 MATCH five\n"
            ),
        )
        .expect("fixture should be written");
        let service = service(
            vec![source(root.path(), "payment-test")],
            LimitsConfig::default(),
        );

        let first = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(2),
            )
            .await
            .expect("first page should succeed");
        assert_eq!(contents(&first), vec!["MATCH one", "MATCH two"]);
        assert_eq!(first.cumulative_usage.pages_returned, 1);
        let first_cursor = first.next_cursor.expect("first cursor should exist");
        assert_eq!(first.results.len(), 2);
        assert!(first.results[0].match_ref.starts_with("mref_"));
        let reference = service
            .match_reference_store()
            .resolve(&first.results[0].match_ref)
            .expect("match reference should resolve");
        assert_eq!(reference.line_number, 1);
        assert_eq!(reference.keyword, "MATCH");

        let second = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(2)
                    .with_cursor(first_cursor),
            )
            .await
            .expect("second page should succeed");
        assert_eq!(contents(&second), vec!["MATCH three", "MATCH four"]);
        assert_eq!(second.cumulative_usage.pages_returned, 2);
        let second_cursor = second.next_cursor.expect("second cursor should exist");

        let third = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(2)
                    .with_cursor(second_cursor),
            )
            .await
            .expect("third page should succeed");
        assert_eq!(contents(&third), vec!["MATCH five"]);
        assert!(third.next_cursor.is_none());
        assert_eq!(third.stop_reason, QueryPageStopReason::Complete);
        assert_eq!(third.cumulative_usage.pages_returned, 3);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cursor_query_mismatch_does_not_consume_cursor() {
        let root = tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            "2026-06-19T14:00:01+09:00 MATCH one\n2026-06-19T14:00:02+09:00 MATCH two\n",
        )
        .expect("fixture should be written");
        let service = service(
            vec![source(root.path(), "payment-test")],
            LimitsConfig::default(),
        );
        let first = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(1),
            )
            .await
            .expect("first page should succeed");
        let cursor = first.next_cursor.expect("cursor should exist");

        let mismatch = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "OTHER")
                    .with_max_results(1)
                    .with_cursor(cursor.clone()),
            )
            .await;
        assert!(matches!(
            mismatch,
            Err(StatefulQueryError::QueryState(
                QueryStateError::QueryMismatch
            ))
        ));

        let retry = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(1)
                    .with_cursor(cursor),
            )
            .await
            .expect("correct retry should succeed");
        assert_eq!(contents(&retry), vec!["MATCH two"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cursor_uses_fixed_file_size_snapshot() {
        let root = tempdir().expect("source root should be created");
        let path = root.path().join("application.log");
        fs::write(
            &path,
            "2026-06-19T14:00:01+09:00 MATCH one\n2026-06-19T14:00:02+09:00 MATCH two\n",
        )
        .expect("fixture should be written");
        let service = service(
            vec![source(root.path(), "payment-test")],
            LimitsConfig::default(),
        );
        let first = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(1),
            )
            .await
            .expect("first page should succeed");
        let cursor = first.next_cursor.expect("cursor should exist");
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("fixture should open for append");
        file.write_all(b"2026-06-19T13:00:00+09:00 MATCH appended\n")
            .expect("append should succeed");

        let second = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(1)
                    .with_cursor(cursor),
            )
            .await
            .expect("second page should succeed");
        assert_eq!(contents(&second), vec!["MATCH two"]);
        assert!(second.next_cursor.is_none());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn scan_byte_limit_does_not_create_misleading_cursor() {
        let root = tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            "2026-06-19T14:00:01+09:00 MATCH one\n2026-06-19T14:00:02+09:00 MATCH two\n",
        )
        .expect("fixture should be written");
        let limits = LimitsConfig {
            max_scan_bytes_per_page: 20,
            ..LimitsConfig::default()
        };
        let service = service(vec![source(root.path(), "payment-test")], limits);

        let page = service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(1),
            )
            .await
            .expect("limited page should succeed");
        assert_eq!(page.stop_reason, QueryPageStopReason::ScanByteLimit);
        assert!(page.next_cursor.is_none());
        assert!(page.continuation_unavailable);
    }

    fn service(sources: Vec<LogSourceConfig>, limits: LimitsConfig) -> StatefulQueryService {
        let registry = Arc::new(
            SourceRegistry::from_config(AppConfig {
                version: CONFIG_VERSION,
                sources,
                limits,
            })
            .expect("registry should build"),
        );
        StatefulQueryService::new(registry).expect("service should build")
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

    fn contents(page: &StatefulQueryPage) -> Vec<String> {
        page.results
            .iter()
            .map(|result| {
                result
                    .content
                    .split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect()
    }
}
