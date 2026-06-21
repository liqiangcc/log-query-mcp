use std::sync::Arc;

use rmcp::{
    Json, ServerHandler,
    handler::server::wrapper::Parameters,
    tool, tool_handler, tool_router,
};
use serde::Serialize;
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::{
    AppConfig, ContextLineResponse, GetLogContextRequest, GetLogContextResponse, LogMatch,
    LogSource, ListLogSourcesResponse, SearchLogsRequest, SearchLogsResponse, SourceRegistry,
    SourceRegistryError, StatefulContextError, StatefulContextRequest, StatefulContextService,
    StatefulQueryError, StatefulQueryRequest, StatefulQueryService, ToolErrorBody,
};

#[derive(Debug)]
pub struct LogQueryRuntime {
    registry: Arc<SourceRegistry>,
    query_service: Arc<StatefulQueryService>,
    context_service: Arc<StatefulContextService>,
    max_response_bytes: usize,
}

impl LogQueryRuntime {
    pub fn from_config(config: AppConfig) -> Result<Self, RuntimeInitError> {
        let registry = Arc::new(SourceRegistry::from_config(config)?);
        let max_response_bytes = registry.limits().max_response_bytes;
        let query_service = Arc::new(StatefulQueryService::new(Arc::clone(&registry))?);
        let context_service = Arc::new(StatefulContextService::from_query_service(
            query_service.as_ref(),
        )?);

        Ok(Self {
            registry,
            query_service,
            context_service,
            max_response_bytes,
        })
    }

    #[must_use]
    pub fn registry(&self) -> &Arc<SourceRegistry> {
        &self.registry
    }

    #[must_use]
    pub const fn max_response_bytes(&self) -> usize {
        self.max_response_bytes
    }
}

#[derive(Clone, Debug)]
pub struct LogQueryServer {
    runtime: Arc<LogQueryRuntime>,
}

impl LogQueryServer {
    #[must_use]
    pub const fn new(runtime: Arc<LogQueryRuntime>) -> Self {
        Self { runtime }
    }

    #[must_use]
    pub fn runtime(&self) -> &Arc<LogQueryRuntime> {
        &self.runtime
    }

    fn checked_json<T: Serialize>(&self, value: T) -> Result<Json<T>, String> {
        let serialized = serde_json::to_vec(&value)
            .map_err(|_| ToolErrorBody::serialization().to_json_text())?;
        if serialized.len() > self.runtime.max_response_bytes {
            return Err(ToolErrorBody::response_limit().to_json_text());
        }
        Ok(Json(value))
    }
}

#[tool_router]
impl LogQueryServer {
    #[tool(
        name = "list_log_sources",
        description = "List enabled log sources. Use returned source_id values with search_logs. Server paths and configured file names are never returned.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn list_log_sources(&self) -> Result<Json<ListLogSourcesResponse>, String> {
        let response = ListLogSourcesResponse {
            sources: self
                .runtime
                .registry
                .list()
                .into_iter()
                .map(|source| LogSource {
                    source_id: source.source_id,
                    name: source.name,
                    description: source.description,
                    service: source.service,
                    environment: source.environment,
                    tags: source.tags,
                })
                .collect(),
        };
        self.checked_json(response)
    }

    #[tool(
        name = "search_logs",
        description = "Search one to ten configured sources using a literal UTF-8 substring. Paths, regular expressions, globs and shell expressions are not accepted. v1 supports only oldest_first ordering.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn search_logs(
        &self,
        Parameters(request): Parameters<SearchLogsRequest>,
        cancellation: CancellationToken,
    ) -> Result<Json<SearchLogsResponse>, String> {
        if request.order != "oldest_first" {
            return Err(ToolErrorBody::new(
                crate::ToolErrorCode::InvalidArgument,
                "v1 supports only oldest_first result ordering",
            )
            .to_json_text());
        }

        let SearchLogsRequest {
            source_ids,
            keyword,
            case_sensitive,
            start_time,
            end_time,
            order: _,
            max_results,
            cursor,
        } = request;
        let mut service_request = StatefulQueryRequest::new(source_ids, keyword)
            .with_case_sensitive(case_sensitive)
            .with_time_range(start_time, end_time)
            .with_max_results(max_results)
            .with_cancellation(cancellation);
        if let Some(cursor) = cursor {
            service_request = service_request.with_cursor(cursor);
        }

        let page = self
            .runtime
            .query_service
            .search(service_request)
            .await
            .map_err(|error| ToolErrorBody::from(error).to_json_text())?;
        let response = SearchLogsResponse {
            results: page
                .results
                .into_iter()
                .map(|result| LogMatch {
                    match_ref: result.match_ref,
                    source_id: result.source_id,
                    file_id: result.file_id,
                    file_name: result.file_name,
                    line_number: result.line_number,
                    timestamp: result.timestamp_rfc3339(),
                    content: result.content,
                    content_truncated: result.content_truncated,
                })
                .collect(),
            truncated: page.truncated,
            next_cursor: page.next_cursor,
        };
        self.checked_json(response)
    }

    #[tool(
        name = "get_log_context",
        description = "Read at most fifty lines on each side of a search match using its opaque match_ref. Arbitrary paths, file identifiers, line numbers and byte offsets are not accepted.",
        annotations(read_only_hint = true, destructive_hint = false, idempotent_hint = true, open_world_hint = false)
    )]
    pub async fn get_log_context(
        &self,
        Parameters(request): Parameters<GetLogContextRequest>,
        cancellation: CancellationToken,
    ) -> Result<Json<GetLogContextResponse>, String> {
        let result = self
            .runtime
            .context_service
            .get_context(
                StatefulContextRequest::new(request.match_ref)
                    .with_lines(request.before_lines, request.after_lines)
                    .with_cancellation(cancellation),
            )
            .await
            .map_err(|error| ToolErrorBody::from(error).to_json_text())?;
        let response = GetLogContextResponse {
            source_id: result.source_id,
            file_id: result.file_id,
            file_name: result.file_name,
            start_line: result.start_line,
            end_line: result.end_line,
            lines: result
                .lines
                .into_iter()
                .map(|line| ContextLineResponse {
                    line_number: line.line_number,
                    content: line.content,
                })
                .collect(),
            truncated: result.truncated,
        };
        self.checked_json(response)
    }
}

#[tool_handler(
    name = "log-query-mcp",
    version = "0.1.0",
    instructions = "Use list_log_sources first, then search_logs with literal identifiers, then get_log_context only for relevant match_ref values. This server never accepts operating-system paths or commands."
)]
impl ServerHandler for LogQueryServer {}

#[derive(Debug, Error)]
pub enum RuntimeInitError {
    #[error(transparent)]
    SourceRegistry(#[from] SourceRegistryError),

    #[error(transparent)]
    QueryService(#[from] StatefulQueryError),

    #[error(transparent)]
    ContextService(#[from] StatefulContextError),
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::{TempDir, tempdir};

    use crate::{
        CONFIG_VERSION, Encoding, LimitsConfig, LogSourceConfig, TimestampRule, ToolErrorCode,
    };

    use super::*;

    fn server(limits: LimitsConfig) -> (TempDir, LogQueryServer) {
        let directory = tempdir().expect("temporary directory should be created");
        fs::write(
            directory.path().join("application.log"),
            concat!(
                "2026-06-19T14:20:01+09:00 INFO before\n",
                "2026-06-19T14:20:02+09:00 ERROR traceId=abc123 failure\n",
                "    at payment::authorize\n",
            ),
        )
        .expect("fixture should be written");
        let runtime = LogQueryRuntime::from_config(AppConfig {
            version: CONFIG_VERSION,
            sources: vec![LogSourceConfig {
                source_id: "payment-test".to_owned(),
                name: "Payment test".to_owned(),
                description: "payment application logs".to_owned(),
                service: "payment".to_owned(),
                environment: "test".to_owned(),
                tags: vec!["java".to_owned()],
                enabled: true,
                encoding: Encoding::Utf8,
                root: directory.path().to_path_buf(),
                files: vec![PathBuf::from("application.log")],
                directories: Vec::new(),
                timestamp_rule: Some(TimestampRule::Rfc3339 { prefix_bytes: 64 }),
            }],
            limits,
        })
        .expect("runtime should build");
        (directory, LogQueryServer::new(Arc::new(runtime)))
    }

    fn search_request(source_id: &str) -> SearchLogsRequest {
        SearchLogsRequest {
            source_ids: vec![source_id.to_owned()],
            keyword: "abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: "oldest_first".to_owned(),
            max_results: 10,
            cursor: None,
        }
    }

    fn parse_error(text: &str) -> ToolErrorBody {
        serde_json::from_str(text).expect("tool error should be JSON")
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delegates_all_tools_to_real_files() {
        let (_directory, server) = server(LimitsConfig::default());

        let sources = server
            .list_log_sources()
            .await
            .expect("list tool should succeed")
            .0;
        assert_eq!(sources.sources.len(), 1);
        assert_eq!(sources.sources[0].source_id, "payment-test");

        let search = server
            .search_logs(
                Parameters(search_request("payment-test")),
                CancellationToken::new(),
            )
            .await
            .expect("search tool should succeed")
            .0;
        assert_eq!(search.results.len(), 1);
        assert!(search.results[0].content.contains("abc123"));
        assert!(search.results[0].match_ref.starts_with("mref_"));

        let context = server
            .get_log_context(
                Parameters(GetLogContextRequest {
                    match_ref: search.results[0].match_ref.clone(),
                    before_lines: 1,
                    after_lines: 1,
                }),
                CancellationToken::new(),
            )
            .await
            .expect("context tool should succeed")
            .0;
        assert_eq!(context.lines.len(), 3);
        assert!(context.lines[1].content.contains("abc123"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn maps_unknown_source_and_unsupported_order_to_stable_errors() {
        let (_directory, server) = server(LimitsConfig::default());

        let error = server
            .search_logs(
                Parameters(search_request("unknown-source")),
                CancellationToken::new(),
            )
            .await
            .expect_err("unknown source should fail");
        assert_eq!(parse_error(&error).code, ToolErrorCode::UnknownSource);
        assert!(!error.contains('/'));

        let mut request = search_request("payment-test");
        request.order = "newest_first".to_owned();
        let error = server
            .search_logs(Parameters(request), CancellationToken::new())
            .await
            .expect_err("unsupported order should fail");
        assert_eq!(parse_error(&error).code, ToolErrorCode::InvalidArgument);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn propagates_rmcp_cancellation_token_to_query_service() {
        let (_directory, server) = server(LimitsConfig::default());
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        let error = server
            .search_logs(
                Parameters(search_request("payment-test")),
                cancellation,
            )
            .await
            .expect_err("cancelled search should fail");
        assert_eq!(parse_error(&error).code, ToolErrorCode::QueryCancelled);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn enforces_complete_serialized_response_limit() {
        let limits = LimitsConfig {
            max_line_bytes: 32,
            max_returned_content_bytes: 64,
            max_response_bytes: 128,
            ..LimitsConfig::default()
        };
        let (_directory, server) = server(limits);

        let error = server
            .search_logs(
                Parameters(search_request("payment-test")),
                CancellationToken::new(),
            )
            .await
            .expect_err("oversized response should fail safely");
        assert_eq!(parse_error(&error).code, ToolErrorCode::ResourceLimit);
    }
}
