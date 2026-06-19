use std::{
    collections::HashMap,
    io::Read,
    path::Path,
    sync::Arc,
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ContextLine, ContextReadError, ContextReadLimits, CursorCandidateFile, CursorSnapshotError,
    GetLogContextRequest, GetLogContextResponse, ListLogSourcesResponse, LogMatch,
    MatchReferenceData, MatchReferenceError, MatchReferenceStore, QueryServiceLimits as _,
    ResultOrder, RuntimeConfigError, ScanExecutor, ScanLimits, ScanMatch, ScanStopReason,
    ScanTaskError, SearchCursorData, SearchCursorError, SearchCursorQuery, SearchCursorStore,
    SearchLogsRequest, SearchLogsResponse, SourceRegistry, TimeFilterDecision, TimeFilterError,
    TimeRange, TimedLogResult, open_cursor_snapshot_reader, read_referenced_context,
    sort_timed_results,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueryServiceLimits {
    pub max_scan_bytes_per_page: u64,
    pub max_returned_content_bytes: usize,
    pub max_response_bytes: usize,
    pub query_timeout: Duration,
    pub max_concurrent_scans: usize,
    pub match_reference_capacity: usize,
    pub match_reference_ttl: Duration,
    pub cursor_capacity: usize,
    pub cursor_ttl: Duration,
    pub context: ContextReadLimits,
}

impl Default for QueryServiceLimits {
    fn default() -> Self {
        Self {
            max_scan_bytes_per_page: 512 * 1024 * 1024,
            max_returned_content_bytes: 512 * 1024,
            max_response_bytes: 1024 * 1024,
            query_timeout: Duration::from_secs(10),
            max_concurrent_scans: 4,
            match_reference_capacity: 10_000,
            match_reference_ttl: Duration::from_secs(10 * 60),
            cursor_capacity: 1_000,
            cursor_ttl: Duration::from_secs(5 * 60),
            context: ContextReadLimits {
                max_returned_content_bytes: 512 * 1024,
                ..ContextReadLimits::default()
            },
        }
    }
}

impl QueryServiceLimits {
    fn validate(self) -> Result<(), QueryError> {
        if self.max_scan_bytes_per_page == 0
            || self.max_returned_content_bytes == 0
            || self.max_response_bytes == 0
            || self.query_timeout == Duration::ZERO
        {
            return Err(QueryError::InvalidLimits);
        }
        if self.max_returned_content_bytes >= self.max_response_bytes {
            return Err(QueryError::InvalidLimits);
        }
        Ok(())
    }
}

#[derive(Debug)]
pub struct QueryService {
    registry: Arc<SourceRegistry>,
    executor: ScanExecutor,
    match_references: Arc<MatchReferenceStore>,
    cursors: Arc<SearchCursorStore>,
    limits: QueryServiceLimits,
}

impl QueryService {
    pub fn new(
        registry: Arc<SourceRegistry>,
        limits: QueryServiceLimits,
    ) -> Result<Self, QueryError> {
        limits.validate()?;
        Ok(Self {
            registry,
            executor: ScanExecutor::new(limits.max_concurrent_scans)?,
            match_references: Arc::new(MatchReferenceStore::new(
                limits.match_reference_capacity,
                limits.match_reference_ttl,
            )?),
            cursors: Arc::new(SearchCursorStore::new(
                limits.cursor_capacity,
                limits.cursor_ttl,
            )?),
            limits,
        })
    }

    #[must_use]
    pub fn list_sources(&self) -> ListLogSourcesResponse {
        ListLogSourcesResponse {
            sources: self.registry.list(),
        }
    }

    pub async fn search(
        &self,
        request: SearchLogsRequest,
    ) -> Result<SearchLogsResponse, QueryError> {
        let query = SearchCursorQuery::from_request(&request)?;
        // This first integrated reader scans files forward. Returning a
        // misleading newest-first page would be worse than rejecting it.
        if matches!(query.order, ResultOrder::NewestFirst) {
            return Err(QueryError::NewestFirstNotIntegrated);
        }
        self.registry.selected(&query.source_ids)?;
        let time_range = TimeRange::from_rfc3339(
            query.start_time.as_deref(),
            query.end_time.as_deref(),
        )?;
        let deadline = Instant::now()
            .checked_add(self.limits.query_timeout)
            .ok_or(QueryError::InvalidLimits)?;
        let cancellation = CancellationToken::new();

        if let Some(cursor) = request.cursor.as_deref() {
            let lease = self.cursors.begin(cursor, &query)?;
            let page = self
                .scan_page(
                    lease.data().clone(),
                    &time_range,
                    deadline,
                    cancellation,
                )
                .await?;
            let next_cursor = lease.commit(page.next_state)?;
            let response = SearchLogsResponse {
                results: page.results,
                truncated: next_cursor.is_some() || page.truncated_without_cursor,
                next_cursor,
            };
            self.ensure_response_size(&response)?;
            return Ok(response);
        }

        let candidates = self.snapshot_candidates(&query.source_ids)?;
        if candidates.is_empty() {
            return Ok(SearchLogsResponse {
                results: Vec::new(),
                truncated: false,
                next_cursor: None,
            });
        }
        let state = SearchCursorData {
            query,
            candidates,
            next_candidate_index: 0,
            next_byte_offset: 0,
            next_line_number: 1,
            files_scanned: 0,
            bytes_scanned: 0,
            results_returned: 0,
        };
        let page = self
            .scan_page(state, &time_range, deadline, cancellation)
            .await?;
        let next_cursor = page
            .next_state
            .map(|state| self.cursors.insert(state))
            .transpose()?;
        let response = SearchLogsResponse {
            results: page.results,
            truncated: next_cursor.is_some() || page.truncated_without_cursor,
            next_cursor,
        };
        self.ensure_response_size(&response)?;
        Ok(response)
    }

    pub async fn get_context(
        &self,
        request: GetLogContextRequest,
    ) -> Result<GetLogContextResponse, QueryError> {
        let reference = self.match_references.resolve(&request.match_ref)?;
        let source = self
            .registry
            .get(&reference.source_id)
            .ok_or_else(|| RuntimeConfigError::UnknownSource(reference.source_id.clone()))?;
        let file_index = source
            .file_index(&reference.relative_path)
            .ok_or(QueryError::ReferenceFileNotConfigured)?;
        let file_id = file_id(&reference.source_id, file_index);
        let file_name = display_file_name(&reference.relative_path);
        let root = source.root();
        let limits = self.limits.context;
        let before_lines = request.before_lines;
        let after_lines = request.after_lines;
        let outcome = tokio::task::spawn_blocking(move || {
            read_referenced_context(
                &root,
                &reference,
                before_lines,
                after_lines,
                limits,
            )
        })
        .await
        .map_err(QueryError::Join)??;

        let start_line = usize::try_from(outcome.start_line).map_err(|_| QueryError::LineOverflow)?;
        let end_line = usize::try_from(outcome.end_line).map_err(|_| QueryError::LineOverflow)?;
        let mut truncated = outcome.before_truncated || outcome.content_truncated;
        let lines = outcome
            .lines
            .into_iter()
            .map(|line| {
                truncated |= line.content_truncated;
                Ok(ContextLine {
                    line_number: usize::try_from(line.line_number)
                        .map_err(|_| QueryError::LineOverflow)?,
                    content: line.content,
                })
            })
            .collect::<Result<Vec<_>, QueryError>>()?;
        let response = GetLogContextResponse {
            source_id: outcome.source_id,
            file_id,
            file_name,
            start_line,
            end_line,
            lines,
            truncated,
        };
        self.ensure_response_size(&response)?;
        Ok(response)
    }

    fn snapshot_candidates(
        &self,
        source_ids: &[String],
    ) -> Result<Vec<CursorCandidateFile>, QueryError> {
        let mut candidates = Vec::new();
        for source_id in source_ids {
            let source = self
                .registry
                .get(source_id)
                .ok_or_else(|| RuntimeConfigError::UnknownSource(source_id.clone()))?;
            for relative_path in source.files() {
                let file = source.root().open_regular_file(relative_path)?;
                candidates.push(CursorCandidateFile {
                    source_id: source_id.clone(),
                    relative_path: relative_path.clone(),
                    file_identity: file.identity(),
                    file_size_at_snapshot: file.size(),
                });
            }
        }
        if candidates.len() > crate::MAX_CURSOR_CANDIDATE_FILES {
            return Err(QueryError::TooManyCandidateFiles);
        }
        Ok(candidates)
    }

    async fn scan_page(
        &self,
        mut state: SearchCursorData,
        time_range: &TimeRange,
        deadline: Instant,
        cancellation: CancellationToken,
    ) -> Result<PageResult, QueryError> {
        state.validate()?;
        let source_order: HashMap<&str, usize> = state
            .query
            .source_ids
            .iter()
            .enumerate()
            .map(|(index, source_id)| (source_id.as_str(), index))
            .collect();
        let mut results = Vec::new();
        let mut page_scan_bytes = 0_u64;
        let mut returned_content_bytes = 0_usize;
        let mut truncated_without_cursor = false;

        while results.len() < state.query.max_results {
            if state.next_candidate_index >= state.candidates.len() {
                break;
            }
            let candidate = state.current_candidate().clone();
            if state.next_byte_offset >= candidate.file_size_at_snapshot {
                if !advance_candidate(&mut state) {
                    break;
                }
                continue;
            }
            let source = self
                .registry
                .get(&candidate.source_id)
                .ok_or_else(|| RuntimeConfigError::UnknownSource(candidate.source_id.clone()))?;
            let root = source.root();
            let reader = open_cursor_snapshot_reader(&root, &state)?;
            let remaining_results = state.query.max_results - results.len();
            let remaining_scan_bytes = self
                .limits
                .max_scan_bytes_per_page
                .saturating_sub(page_scan_bytes);
            let remaining_content_bytes = self
                .limits
                .max_returned_content_bytes
                .saturating_sub(returned_content_bytes);
            if remaining_scan_bytes == 0 || remaining_content_bytes == 0 {
                truncated_without_cursor = true;
                break;
            }

            let scan_limits = ScanLimits {
                max_scan_bytes: remaining_scan_bytes,
                max_results: remaining_results,
                max_line_bytes: ScanLimits::default().max_line_bytes,
                max_returned_content_bytes: remaining_content_bytes,
                read_buffer_bytes: ScanLimits::default().read_buffer_bytes,
            };
            let scan_request = crate::ScanRequest::new(state.query.keyword.clone())
                .with_case_sensitive(state.query.case_sensitive)
                .with_limits(scan_limits)
                .with_deadline(deadline)
                .with_cancellation(cancellation.clone());
            let base_offset = state.next_byte_offset;
            let base_line = state.next_line_number;
            let outcome = self.executor.scan(reader, scan_request).await?;
            page_scan_bytes = page_scan_bytes.saturating_add(outcome.bytes_scanned);
            state.bytes_scanned = state.bytes_scanned.saturating_add(outcome.bytes_scanned);
            state.next_byte_offset = state.next_byte_offset.saturating_add(outcome.bytes_scanned);
            state.next_line_number = state.next_line_number.saturating_add(outcome.lines_scanned);

            let source_index = *source_order
                .get(candidate.source_id.as_str())
                .ok_or(QueryError::InternalInvariant)?;
            let file_index = source
                .file_index(&candidate.relative_path)
                .ok_or(QueryError::InternalInvariant)?;
            for scan_match in outcome.results {
                let scan_match = rebase_match(scan_match, base_offset, base_line)?;
                let timestamp = source
                    .timestamp_rule()
                    .and_then(|rule| rule.parse_line(&scan_match.content));
                match time_range.classify(timestamp.as_ref()) {
                    TimeFilterDecision::OutOfRange => continue,
                    TimeFilterDecision::InRange
                    | TimeFilterDecision::UnknownTimestamp
                    | TimeFilterDecision::MalformedTimestamp => {}
                }
                let reference = MatchReferenceData::from_scan_match(
                    candidate.source_id.clone(),
                    candidate.relative_path.clone(),
                    candidate.file_identity,
                    candidate.file_size_at_snapshot,
                    state.query.keyword.clone(),
                    state.query.case_sensitive,
                    &scan_match,
                )?;
                let match_ref = self.match_references.insert(reference)?;
                returned_content_bytes =
                    returned_content_bytes.saturating_add(scan_match.content.len());
                results.push(TimedLogResult {
                    timestamp: timestamp.clone(),
                    source_index,
                    file_index,
                    line_number: scan_match.line_number,
                    value: LogMatch {
                        match_ref,
                        source_id: candidate.source_id.clone(),
                        file_id: file_id(&candidate.source_id, file_index),
                        file_name: display_file_name(&candidate.relative_path),
                        line_number: usize::try_from(scan_match.line_number)
                            .map_err(|_| QueryError::LineOverflow)?,
                        timestamp: timestamp.map(|value| value.to_rfc3339()),
                        content: scan_match.content,
                        content_truncated: scan_match.content_truncated,
                    },
                });
                if results.len() >= state.query.max_results {
                    break;
                }
            }
            state.results_returned = state.results_returned.saturating_add(results.len());

            match outcome.stop_reason {
                ScanStopReason::Complete => {
                    if !advance_candidate(&mut state) {
                        state.next_candidate_index = state.candidates.len();
                        break;
                    }
                }
                ScanStopReason::ResultLimit => {
                    if state.next_byte_offset >= candidate.file_size_at_snapshot
                        && !advance_candidate(&mut state)
                    {
                        state.next_candidate_index = state.candidates.len();
                        break;
                    }
                }
                ScanStopReason::ScanByteLimit | ScanStopReason::ReturnedContentByteLimit => {
                    truncated_without_cursor = true;
                    break;
                }
                ScanStopReason::Cancelled => return Err(QueryError::Cancelled),
                ScanStopReason::DeadlineExceeded => return Err(QueryError::DeadlineExceeded),
            }
        }

        sort_timed_results(&mut results, state.query.order);
        let values = results.into_iter().map(|result| result.value).collect();
        let next_state = if !truncated_without_cursor
            && state.next_candidate_index < state.candidates.len()
        {
            Some(state)
        } else {
            None
        };
        Ok(PageResult {
            results: values,
            next_state,
            truncated_without_cursor,
        })
    }

    fn ensure_response_size<T: serde::Serialize>(&self, response: &T) -> Result<(), QueryError> {
        let bytes = serde_json::to_vec(response).map_err(QueryError::SerializeResponse)?;
        if bytes.len() > self.limits.max_response_bytes {
            return Err(QueryError::ResponseTooLarge);
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PageResult {
    results: Vec<LogMatch>,
    next_state: Option<SearchCursorData>,
    truncated_without_cursor: bool,
}

fn advance_candidate(state: &mut SearchCursorData) -> bool {
    let next = state.next_candidate_index.saturating_add(1);
    if next >= state.candidates.len() {
        return false;
    }
    state.next_candidate_index = next;
    state.files_scanned = next;
    state.next_byte_offset = 0;
    state.next_line_number = 1;
    true
}

fn rebase_match(
    mut scan_match: ScanMatch,
    base_offset: u64,
    base_line: u64,
) -> Result<ScanMatch, QueryError> {
    scan_match.line_number = base_line
        .checked_add(scan_match.line_number.saturating_sub(1))
        .ok_or(QueryError::LineOverflow)?;
    scan_match.line_start_offset = base_offset
        .checked_add(scan_match.line_start_offset)
        .ok_or(QueryError::OffsetOverflow)?;
    scan_match.match_byte_offset = base_offset
        .checked_add(scan_match.match_byte_offset)
        .ok_or(QueryError::OffsetOverflow)?;
    Ok(scan_match)
}

fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("log-file")
        .to_owned()
}

fn file_id(source_id: &str, file_index: usize) -> String {
    format!("file-{source_id}-{file_index}")
}

#[derive(Debug, Error)]
pub enum QueryError {
    #[error("query service limits are invalid")]
    InvalidLimits,

    #[error("newest_first is not yet supported by the integrated forward-only scanner")]
    NewestFirstNotIntegrated,

    #[error("query selected too many candidate files")]
    TooManyCandidateFiles,

    #[error("query was cancelled")]
    Cancelled,

    #[error("query exceeded its execution deadline")]
    DeadlineExceeded,

    #[error("query response exceeds the service response limit")]
    ResponseTooLarge,

    #[error("match reference no longer points to a configured file")]
    ReferenceFileNotConfigured,

    #[error("log line number cannot be represented in the MCP response")]
    LineOverflow,

    #[error("log byte offset overflowed")]
    OffsetOverflow,

    #[error("internal query state is inconsistent")]
    InternalInvariant,

    #[error("runtime source configuration rejected the query")]
    Config(#[from] RuntimeConfigError),

    #[error("safe log file access failed")]
    SafeOpen(#[from] crate::SafeOpenError),

    #[error("log scan failed")]
    Scan(#[from] ScanTaskError),

    #[error("search cursor is invalid or expired")]
    Cursor(#[from] SearchCursorError),

    #[error("search cursor snapshot cannot be reopened")]
    CursorSnapshot(#[from] CursorSnapshotError),

    #[error("match reference is invalid or expired")]
    MatchReference(#[from] MatchReferenceError),

    #[error("context read failed")]
    Context(#[from] ContextReadError),

    #[error("time range is invalid")]
    TimeFilter(#[from] TimeFilterError),

    #[error("blocking query task failed")]
    Join(#[from] tokio::task::JoinError),

    #[error("MCP response serialization failed")]
    SerializeResponse(#[source] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::{
        LogSourceConfig, ServiceConfig, TimestampRuleConfig,
    };

    use super::*;

    fn request(max_results: usize) -> SearchLogsRequest {
        SearchLogsRequest {
            source_ids: vec!["payment-test".to_owned()],
            keyword: "abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results,
            cursor: None,
        }
    }

    fn registry(root: PathBuf) -> Arc<SourceRegistry> {
        Arc::new(
            SourceRegistry::from_config(
                ServiceConfig {
                    sources: vec![LogSourceConfig {
                        source_id: "payment-test".to_owned(),
                        name: "payment test".to_owned(),
                        description: String::new(),
                        service: "payment".to_owned(),
                        environment: "test".to_owned(),
                        tags: vec!["java".to_owned()],
                        root,
                        files: vec![PathBuf::from("application.log")],
                        timestamp_rule: Some(TimestampRuleConfig::Rfc3339 {
                            prefix_bytes: 64,
                        }),
                    }],
                },
                ".",
            )
            .expect("registry should load"),
        )
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn searches_real_file_pages_and_reads_context() {
        let directory = tempdir().expect("temporary directory should be created");
        let content = concat!(
            "2026-06-19T14:20:01+09:00 INFO before\n",
            "2026-06-19T14:20:02+09:00 ERROR traceId=abc123 first\n",
            "    at payment::authorize\n",
            "2026-06-19T14:20:03+09:00 ERROR traceId=abc123 second\n",
        );
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let service = QueryService::new(registry(directory.path().to_path_buf()), Default::default())
            .expect("service should start");

        let first = service.search(request(1)).await.expect("search should succeed");
        assert_eq!(first.results.len(), 1);
        assert!(first.next_cursor.is_some());
        assert!(first.results[0].content.contains("first"));

        let context = service
            .get_context(GetLogContextRequest {
                match_ref: first.results[0].match_ref.clone(),
                before_lines: 1,
                after_lines: 1,
            })
            .await
            .expect("context should succeed");
        assert_eq!(context.lines.len(), 3);
        assert!(context.lines[1].content.contains("abc123"));

        let mut second_request = request(1);
        second_request.cursor = first.next_cursor;
        let second = service
            .search(second_request)
            .await
            .expect("second page should succeed");
        assert_eq!(second.results.len(), 1);
        assert!(second.results[0].content.contains("second"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn applies_time_range_to_real_matches() {
        let directory = tempdir().expect("temporary directory should be created");
        let content = concat!(
            "2026-06-19T14:20:02+09:00 ERROR traceId=abc123 early\n",
            "2026-06-19T14:20:03+09:00 ERROR traceId=abc123 in-range\n",
        );
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let service = QueryService::new(registry(directory.path().to_path_buf()), Default::default())
            .expect("service should start");
        let mut search = request(10);
        search.start_time = Some("2026-06-19T14:20:03+09:00".to_owned());
        search.end_time = Some("2026-06-19T14:20:04+09:00".to_owned());

        let response = service.search(search).await.expect("search should succeed");
        assert_eq!(response.results.len(), 1);
        assert!(response.results[0].content.contains("in-range"));
    }
}
