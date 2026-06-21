use std::sync::Arc;

use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};

use crate::{
    GetLogContextRequest, GetLogContextResponse, ListLogSourcesRequest, ListLogSourcesResponse,
    LogSource, SearchLogsRequest, SearchLogsResponse, SourceRegistry, StatefulContextRequest,
    StatefulContextService, StatefulQueryRequest, StatefulQueryService, ToolError,
    serialize_with_limit,
};

const SERVER_NAME: &str = "log-query-mcp";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Clone)]
pub struct LogQueryMcpServer {
    query_service: Arc<StatefulQueryService>,
    context_service: Arc<StatefulContextService>,
    max_response_bytes: usize,
    tool_router: ToolRouter<Self>,
}

impl LogQueryMcpServer {
    pub fn new(query_service: StatefulQueryService) -> Result<Self, ToolError> {
        let max_response_bytes = query_service.registry().limits().max_response_bytes;
        let context_service =
            StatefulContextService::from_query_service(&query_service).map_err(ToolError::from)?;
        Ok(Self {
            query_service: Arc::new(query_service),
            context_service: Arc::new(context_service),
            max_response_bytes,
            tool_router: Self::tool_router(),
        })
    }

    pub fn from_registry(registry: Arc<SourceRegistry>) -> Result<Self, ToolError> {
        Self::new(StatefulQueryService::new(registry).map_err(ToolError::from)?)
    }

    #[must_use]
    pub fn query_service(&self) -> &Arc<StatefulQueryService> {
        &self.query_service
    }

    #[must_use]
    pub fn context_service(&self) -> &Arc<StatefulContextService> {
        &self.context_service
    }

    #[must_use]
    pub fn list_log_sources_result(&self) -> CallToolResult {
        self.success_json(&ListLogSourcesResponse {
            sources: self
                .query_service
                .registry()
                .list()
                .into_iter()
                .map(LogSource::from)
                .collect(),
        })
    }

    pub async fn search_logs_result(&self, request: SearchLogsRequest) -> CallToolResult {
        let mut query_request = StatefulQueryRequest::new(request.source_ids, request.keyword)
            .with_case_sensitive(request.case_sensitive);
        query_request = query_request.with_time_range(request.start_time, request.end_time);
        if let Some(max_results) = request.max_results {
            query_request = query_request.with_max_results(max_results);
        }
        if let Some(cursor) = request.cursor {
            query_request = query_request.with_cursor(cursor);
        }

        match self.query_service.search(query_request).await {
            Ok(page) => self.success_json(&SearchLogsResponse::from(page)),
            Err(error) => self.error_json(ToolError::from(error)),
        }
    }

    pub async fn get_log_context_result(&self, request: GetLogContextRequest) -> CallToolResult {
        let context_request = StatefulContextRequest::new(request.match_ref)
            .with_lines(request.before_lines, request.after_lines);
        match self.context_service.get_context(context_request).await {
            Ok(context) => self.success_json(&GetLogContextResponse::from(context)),
            Err(error) => self.error_json(ToolError::from(error)),
        }
    }

    fn success_json<T: serde::Serialize>(&self, value: &T) -> CallToolResult {
        match serialize_with_limit(value, self.max_response_bytes) {
            Ok(json) => CallToolResult::success(vec![Content::text(json)]),
            Err(error) => self.error_json(error),
        }
    }

    fn error_json(&self, error: ToolError) -> CallToolResult {
        CallToolResult::error(vec![Content::text(tool_error_json(error))])
    }
}

fn tool_error_json(error: ToolError) -> String {
    error.to_json_string().unwrap_or_else(|_| {
        r#"{"code":"INTERNAL_ERROR","message":"an internal error occurred; check the service logs","retryable":true}"#
            .to_owned()
    })
}

#[tool_router(router = tool_router)]
impl LogQueryMcpServer {
    #[tool(
        name = "list_log_sources",
        description = "List configured and enabled log sources",
        output_schema = rmcp::handler::server::tool::schema_for_type::<ListLogSourcesResponse>()
    )]
    fn list_log_sources(
        &self,
        Parameters(_request): Parameters<ListLogSourcesRequest>,
    ) -> CallToolResult {
        self.list_log_sources_result()
    }

    #[tool(
        name = "search_logs",
        description = "Search configured log sources for a literal keyword",
        output_schema = rmcp::handler::server::tool::schema_for_type::<SearchLogsResponse>()
    )]
    async fn search_logs(
        &self,
        Parameters(request): Parameters<SearchLogsRequest>,
    ) -> CallToolResult {
        self.search_logs_result(request).await
    }

    #[tool(
        name = "get_log_context",
        description = "Read bounded context around a match_ref returned by search_logs",
        output_schema = rmcp::handler::server::tool::schema_for_type::<GetLogContextResponse>()
    )]
    async fn get_log_context(
        &self,
        Parameters(request): Parameters<GetLogContextRequest>,
    ) -> CallToolResult {
        self.get_log_context_result(request).await
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for LogQueryMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Read-only log search tools for configured Linux log sources")
            .with_server_info(Implementation::new(SERVER_NAME, SERVER_VERSION))
    }
}
