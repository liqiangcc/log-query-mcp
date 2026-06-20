use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    ContextExecution, ContextExecutor, ContextLine, ContextReadError, ContextReadLimits,
    ContextTaskError, MatchReferenceStore, QueryStateError, SourceRegistry, SourceRegistryError,
    StatefulQueryService, read_referenced_context,
};

const MAX_MATCH_REFERENCE_CHARS: usize = 512;

#[derive(Debug, Clone)]
pub struct StatefulContextRequest {
    pub match_ref: String,
    pub before_lines: usize,
    pub after_lines: usize,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
}

impl StatefulContextRequest {
    #[must_use]
    pub fn new(match_ref: impl Into<String>) -> Self {
        Self {
            match_ref: match_ref.into(),
            before_lines: 0,
            after_lines: 0,
            deadline: None,
            cancellation: CancellationToken::new(),
        }
    }

    #[must_use]
    pub fn with_lines(mut self, before_lines: usize, after_lines: usize) -> Self {
        self.before_lines = before_lines;
        self.after_lines = after_lines;
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatefulContextResult {
    pub source_id: String,
    pub file_id: String,
    pub file_name: String,
    pub start_line: u64,
    pub end_line: u64,
    pub lines: Vec<ContextLine>,
    pub truncated: bool,
    pub before_truncated: bool,
    pub after_truncated: bool,
    pub returned_content_bytes: usize,
    pub bytes_scanned: u64,
}

#[derive(Clone, Debug)]
pub struct StatefulContextService {
    registry: Arc<SourceRegistry>,
    match_references: Arc<MatchReferenceStore>,
    executor: ContextExecutor,
}

impl StatefulContextService {
    pub fn new(
        registry: Arc<SourceRegistry>,
        match_references: Arc<MatchReferenceStore>,
    ) -> Result<Self, StatefulContextError> {
        let executor = ContextExecutor::new(registry.limits().max_concurrent_scans)?;
        Ok(Self {
            registry,
            match_references,
            executor,
        })
    }

    pub fn from_query_service(
        service: &StatefulQueryService,
    ) -> Result<Self, StatefulContextError> {
        Self::new(
            Arc::clone(service.registry()),
            Arc::clone(service.match_reference_store()),
        )
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<SourceRegistry> {
        &self.registry
    }

    pub async fn get_context(
        &self,
        request: StatefulContextRequest,
    ) -> Result<StatefulContextResult, StatefulContextError> {
        validate_request(&request, self.registry.limits().max_context_lines_per_side)?;
        let deadline = effective_deadline(
            request.deadline,
            self.registry.limits().query_timeout_millis,
        )?;
        if request.cancellation.is_cancelled() {
            return Err(StatefulContextError::Cancelled);
        }
        if Instant::now() >= deadline {
            return Err(StatefulContextError::DeadlineExceeded);
        }

        let reference = self.match_references.resolve(&request.match_ref)?;
        let source = self
            .registry
            .get(&reference.source_id)
            .ok_or_else(|| SourceRegistryError::UnknownSource(reference.source_id.clone()))?;
        let limits = ContextReadLimits::from_service_limits(self.registry.limits());
        let cancellation = request.cancellation.clone();
        let worker_cancellation = cancellation.clone();
        let worker_reference = reference.clone();
        let worker_source = Arc::clone(&source);
        let before_lines = request.before_lines;
        let after_lines = request.after_lines;

        let execution = self
            .executor
            .execute(cancellation, Some(deadline), move || {
                read_referenced_context(
                    &worker_source,
                    &worker_reference,
                    before_lines,
                    after_lines,
                    limits,
                    &worker_cancellation,
                    Some(deadline),
                )
            })
            .await?;
        let outcome = match execution {
            ContextExecution::Complete(result) => result?,
            ContextExecution::Cancelled => return Err(StatefulContextError::Cancelled),
            ContextExecution::DeadlineExceeded => {
                return Err(StatefulContextError::DeadlineExceeded);
            }
        };

        Ok(StatefulContextResult {
            source_id: reference.source_id,
            file_id: reference.file_id,
            file_name: reference.relative_path.to_string_lossy().into_owned(),
            start_line: outcome.start_line,
            end_line: outcome.end_line,
            lines: outcome.lines,
            truncated: outcome.truncated,
            before_truncated: outcome.before_truncated,
            after_truncated: outcome.after_truncated,
            returned_content_bytes: outcome.returned_content_bytes,
            bytes_scanned: outcome.bytes_scanned,
        })
    }
}

fn validate_request(
    request: &StatefulContextRequest,
    max_lines_per_side: usize,
) -> Result<(), StatefulContextError> {
    let match_ref_chars = request.match_ref.chars().count();
    if match_ref_chars == 0 || match_ref_chars > MAX_MATCH_REFERENCE_CHARS {
        return Err(StatefulContextError::InvalidArgument(
            "match_ref length is outside the v1 contract",
        ));
    }
    if request.before_lines > max_lines_per_side || request.after_lines > max_lines_per_side {
        return Err(StatefulContextError::InvalidArgument(
            "context line count exceeds the configured limit",
        ));
    }
    Ok(())
}

fn effective_deadline(
    requested: Option<Instant>,
    timeout_millis: u64,
) -> Result<Instant, StatefulContextError> {
    let service_deadline = Instant::now()
        .checked_add(Duration::from_millis(timeout_millis))
        .ok_or(StatefulContextError::DeadlineOverflow)?;
    Ok(requested.map_or(service_deadline, |requested| {
        requested.min(service_deadline)
    }))
}

#[derive(Debug, Error)]
pub enum StatefulContextError {
    #[error("invalid context request: {0}")]
    InvalidArgument(&'static str),

    #[error("context deadline cannot be represented")]
    DeadlineOverflow,

    #[error("context request was cancelled")]
    Cancelled,

    #[error("context deadline was exceeded")]
    DeadlineExceeded,

    #[error(transparent)]
    QueryState(#[from] QueryStateError),

    #[error(transparent)]
    SourceRegistry(#[from] SourceRegistryError),

    #[error(transparent)]
    ContextRead(#[from] ContextReadError),

    #[error(transparent)]
    ContextTask(#[from] ContextTaskError),
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::tempdir;

    use crate::{
        AppConfig, CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, StatefulQueryRequest,
        TimestampRule,
    };

    use super::*;

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn resolves_search_match_reference_and_reads_bounded_context() {
        let root = tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            concat!(
                "2026-06-19T14:00:00+09:00 before\n",
                "2026-06-19T14:00:01+09:00 MATCH failure\n",
                "    at payment::authorize\n",
                "Caused by: forbidden\n",
                "after\n"
            ),
        )
        .expect("fixture should be written");
        let query_service = service(root.path());
        let context_service = StatefulContextService::from_query_service(&query_service)
            .expect("context service should build");

        let page = query_service
            .search(
                StatefulQueryRequest::new(vec!["payment-test".to_owned()], "MATCH")
                    .with_max_results(10),
            )
            .await
            .expect("search should succeed");
        let result = context_service
            .get_context(
                StatefulContextRequest::new(page.results[0].match_ref.clone()).with_lines(1, 2),
            )
            .await
            .expect("context should succeed");

        assert_eq!(result.start_line, 1);
        assert_eq!(result.end_line, 4);
        assert_eq!(result.lines.len(), 4);
        assert!(result.lines[1].is_match_line);
        assert!(result.lines[1].content.contains("MATCH failure"));
        assert_eq!(result.lines[2].content, "    at payment::authorize");
        assert_eq!(result.lines[3].content, "Caused by: forbidden");
        assert!(!result.truncated);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn detects_file_replacement_after_search() {
        let root = tempdir().expect("source root should be created");
        let path = root.path().join("application.log");
        let rotated = root.path().join("application.log.1");
        fs::write(&path, "2026-06-19T14:00:01+09:00 MATCH original\n")
            .expect("fixture should be written");
        let query_service = service(root.path());
        let context_service = StatefulContextService::from_query_service(&query_service)
            .expect("context service should build");
        let page = query_service
            .search(StatefulQueryRequest::new(
                vec!["payment-test".to_owned()],
                "MATCH",
            ))
            .await
            .expect("search should succeed");

        fs::rename(&path, &rotated).expect("fixture should rotate");
        fs::write(&path, "2026-06-19T14:00:02+09:00 MATCH replacement\n")
            .expect("replacement should be written");

        assert!(matches!(
            context_service
                .get_context(StatefulContextRequest::new(
                    page.results[0].match_ref.clone()
                ))
                .await,
            Err(StatefulContextError::ContextRead(
                ContextReadError::FileChanged
            ))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn rejects_pre_cancelled_context_request() {
        let root = tempdir().expect("source root should be created");
        fs::write(
            root.path().join("application.log"),
            "2026-06-19T14:00:01+09:00 MATCH failure\n",
        )
        .expect("fixture should be written");
        let query_service = service(root.path());
        let context_service = StatefulContextService::from_query_service(&query_service)
            .expect("context service should build");
        let page = query_service
            .search(StatefulQueryRequest::new(
                vec!["payment-test".to_owned()],
                "MATCH",
            ))
            .await
            .expect("search should succeed");
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            context_service
                .get_context(
                    StatefulContextRequest::new(page.results[0].match_ref.clone())
                        .with_cancellation(cancellation)
                )
                .await,
            Err(StatefulContextError::Cancelled)
        ));
    }

    fn service(root: &std::path::Path) -> StatefulQueryService {
        let registry = Arc::new(
            SourceRegistry::from_config(AppConfig {
                version: CONFIG_VERSION,
                sources: vec![LogSourceConfig {
                    source_id: "payment-test".to_owned(),
                    name: "Payment".to_owned(),
                    description: String::new(),
                    service: "payment".to_owned(),
                    environment: "test".to_owned(),
                    tags: Vec::new(),
                    enabled: true,
                    encoding: Encoding::Utf8,
                    root: root.to_path_buf(),
                    files: vec![PathBuf::from("application.log")],
                    directories: Vec::new(),
                    timestamp_rule: Some(TimestampRule::Rfc3339 { prefix_bytes: 64 }),
                }],
                limits: LimitsConfig::default(),
            })
            .expect("registry should build"),
        );
        StatefulQueryService::new(registry).expect("query service should build")
    }
}
