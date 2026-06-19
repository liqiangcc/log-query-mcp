use std::sync::Arc;

use rmcp::{
    Json,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};

use crate::{
    GetLogContextRequest, GetLogContextResponse, ListLogSourcesResponse, QueryError, QueryService,
    SearchLogsRequest, SearchLogsResponse,
};

#[derive(Clone)]
pub struct LogQueryServer {
    tool_router: ToolRouter<Self>,
    query_service: Arc<QueryService>,
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for LogQueryServer {}

#[tool_router(router = tool_router)]
impl LogQueryServer {
    #[must_use]
    pub fn new(query_service: Arc<QueryService>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            query_service,
        }
    }

    #[tool(
        name = "list_log_sources",
        description = "List configured log sources. Use source_id values from this result in search_logs. Server file-system paths are never returned."
    )]
    pub async fn list_log_sources(&self) -> Json<ListLogSourcesResponse> {
        Json(self.query_service.list_sources())
    }

    #[tool(
        name = "search_logs",
        description = "Search one to ten configured log sources using a literal UTF-8 substring. Use source_id values from list_log_sources. Paths, regexes, globs and shell expressions are not accepted."
    )]
    pub async fn search_logs(
        &self,
        Parameters(request): Parameters<SearchLogsRequest>,
    ) -> Result<Json<SearchLogsResponse>, String> {
        self.query_service
            .search(request)
            .await
            .map(Json)
            .map_err(tool_error)
    }

    #[tool(
        name = "get_log_context",
        description = "Read up to fifty lines on each side of a prior search result using its opaque match_ref. Arbitrary file paths and line numbers are not accepted."
    )]
    pub async fn get_log_context(
        &self,
        Parameters(request): Parameters<GetLogContextRequest>,
    ) -> Result<Json<GetLogContextResponse>, String> {
        self.query_service
            .get_context(request)
            .await
            .map(Json)
            .map_err(tool_error)
    }
}

fn tool_error(error: QueryError) -> String {
    error.to_string()
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use tempfile::{TempDir, tempdir};

    use crate::{
        LogSourceConfig, QueryServiceLimits, ResultOrder, ServiceConfig, SourceRegistry,
        TimestampRuleConfig,
    };

    use super::*;

    fn server() -> (TempDir, LogQueryServer) {
        let directory = tempdir().expect("temporary directory should be created");
        let content = concat!(
            "2026-06-19T14:20:01+09:00 INFO before\n",
            "2026-06-19T14:20:02+09:00 ERROR traceId=abc123 failure\n",
            "    at payment::authorize\n",
        );
        fs::write(directory.path().join("application.log"), content)
            .expect("fixture should be written");
        let registry = SourceRegistry::from_config(
            ServiceConfig {
                sources: vec![LogSourceConfig {
                    source_id: "payment-test".to_owned(),
                    name: "payment test".to_owned(),
                    description: "payment application logs".to_owned(),
                    service: "payment".to_owned(),
                    environment: "test".to_owned(),
                    tags: vec!["java".to_owned()],
                    root: directory.path().to_path_buf(),
                    files: vec![PathBuf::from("application.log")],
                    timestamp_rule: Some(TimestampRuleConfig::Rfc3339 { prefix_bytes: 64 }),
                }],
            },
            ".",
        )
        .expect("registry should load");
        let query_service = QueryService::new(Arc::new(registry), QueryServiceLimits::default())
            .expect("query service should start");

        (directory, LogQueryServer::new(Arc::new(query_service)))
    }

    fn request(source_id: &str) -> SearchLogsRequest {
        SearchLogsRequest {
            source_ids: vec![source_id.to_owned()],
            keyword: "abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: ResultOrder::OldestFirst,
            max_results: 10,
            cursor: None,
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn delegates_tools_to_real_files() {
        let (_directory, server) = server();

        let sources = server.list_log_sources().await.0;
        assert_eq!(sources.sources.len(), 1);
        assert_eq!(sources.sources[0].source_id, "payment-test");

        let search = server
            .search_logs(Parameters(request("payment-test")))
            .await
            .expect("search tool should succeed")
            .0;
        assert_eq!(search.results.len(), 1);
        assert!(search.results[0].content.contains("abc123"));
        assert!(search.results[0].match_ref.starts_with("mref_"));

        let context = server
            .get_log_context(Parameters(GetLogContextRequest {
                match_ref: search.results[0].match_ref.clone(),
                before_lines: 1,
                after_lines: 1,
            }))
            .await
            .expect("context tool should succeed")
            .0;
        assert_eq!(context.lines.len(), 3);
        assert!(context.lines[1].content.contains("abc123"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn returns_sanitized_tool_error_for_unknown_source() {
        let (_directory, server) = server();
        let error = server
            .search_logs(Parameters(request("unknown-source")))
            .await
            .expect_err("unknown source should fail");

        assert!(error.contains("configuration"));
        assert!(!error.contains('/'));
    }
}
