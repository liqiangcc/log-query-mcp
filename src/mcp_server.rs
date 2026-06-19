use std::collections::HashSet;

use rmcp::{
    Json,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    tool, tool_handler, tool_router,
};

use crate::{
    ContextLine, GetLogContextRequest, GetLogContextResponse, ListLogSourcesResponse, LogMatch,
    LogSource, ResultOrder, SearchLogsRequest, SearchLogsResponse,
};

const MAX_SOURCES: usize = 10;
const MAX_KEYWORD_CHARS: usize = 256;
const MAX_RESULTS: usize = 200;
const MAX_CONTEXT_LINES_PER_SIDE: usize = 50;

#[derive(Debug, Clone, Copy)]
struct MockRecord {
    match_ref: &'static str,
    source_id: &'static str,
    file_id: &'static str,
    file_name: &'static str,
    line_number: usize,
    timestamp: &'static str,
    content: &'static str,
}

const MOCK_RECORDS: &[MockRecord] = &[
    MockRecord {
        match_ref: "match-payment-4",
        source_id: "payment-test",
        file_id: "file-payment-application",
        file_name: "application.log",
        line_number: 4,
        timestamp: "2026-06-19T14:20:03.125+09:00",
        content: "ERROR traceId=abc123 orderId=10001 PaymentAuthException: channel returned 403",
    },
    MockRecord {
        match_ref: "match-order-3",
        source_id: "order-test",
        file_id: "file-order-application",
        file_name: "application.log",
        line_number: 3,
        timestamp: "2026-06-19T14:20:03.200+09:00",
        content: "ERROR traceId=abc123 orderId=10001 payment failed",
    },
    MockRecord {
        match_ref: "match-payment-8",
        source_id: "payment-test",
        file_id: "file-payment-application",
        file_name: "application.log",
        line_number: 8,
        timestamp: "2026-06-19T14:21:11.000+09:00",
        content: "INFO traceId=def456 orderId=10002 payment succeeded",
    },
];

#[derive(Clone)]
pub struct LogQueryServer {
    tool_router: ToolRouter<Self>,
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for LogQueryServer {}

impl Default for LogQueryServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router(router = tool_router)]
impl LogQueryServer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "list_log_sources",
        description = "List configured log sources. Server file-system paths are never returned."
    )]
    pub async fn list_log_sources(&self) -> Json<ListLogSourcesResponse> {
        Json(Self::list_sources_response())
    }

    #[tool(
        name = "search_logs",
        description = "Search one or more configured log sources using a literal UTF-8 substring. Paths, regexes and shell expressions are not accepted."
    )]
    pub async fn search_logs(
        &self,
        Parameters(request): Parameters<SearchLogsRequest>,
    ) -> Result<Json<SearchLogsResponse>, String> {
        Self::search_response(request).map(Json)
    }

    #[tool(
        name = "get_log_context",
        description = "Read a limited number of lines around a prior search result using its opaque match_ref. Arbitrary file paths and line numbers are not accepted."
    )]
    pub async fn get_log_context(
        &self,
        Parameters(request): Parameters<GetLogContextRequest>,
    ) -> Result<Json<GetLogContextResponse>, String> {
        Self::context_response(request).map(Json)
    }

    fn list_sources_response() -> ListLogSourcesResponse {
        ListLogSourcesResponse {
            sources: vec![
                LogSource {
                    source_id: "payment-test".to_owned(),
                    name: "支付服务测试环境".to_owned(),
                    description: "payment-service application logs".to_owned(),
                    service: "payment-service".to_owned(),
                    environment: "test".to_owned(),
                    tags: vec!["payment".to_owned(), "java".to_owned()],
                },
                LogSource {
                    source_id: "order-test".to_owned(),
                    name: "订单服务测试环境".to_owned(),
                    description: "order-service application logs".to_owned(),
                    service: "order-service".to_owned(),
                    environment: "test".to_owned(),
                    tags: vec!["order".to_owned(), "java".to_owned()],
                },
            ],
        }
    }

    fn search_response(request: SearchLogsRequest) -> Result<SearchLogsResponse, String> {
        Self::validate_search_request(&request)?;

        if request.cursor.is_some() {
            return Err("cursor pagination is not implemented in the first POC".to_owned());
        }

        let selected_sources: HashSet<&str> =
            request.source_ids.iter().map(String::as_str).collect();
        let case_sensitive = request.case_sensitive.unwrap_or(false);
        let max_results = request.max_results.unwrap_or(50);
        let keyword = if case_sensitive {
            request.keyword.clone()
        } else {
            request.keyword.to_lowercase()
        };

        let mut matches: Vec<LogMatch> = MOCK_RECORDS
            .iter()
            .filter(|record| selected_sources.contains(record.source_id))
            .filter(|record| {
                if case_sensitive {
                    record.content.contains(&keyword)
                } else {
                    record.content.to_lowercase().contains(&keyword)
                }
            })
            .map(|record| LogMatch {
                match_ref: record.match_ref.to_owned(),
                source_id: record.source_id.to_owned(),
                file_id: record.file_id.to_owned(),
                file_name: record.file_name.to_owned(),
                line_number: record.line_number,
                timestamp: Some(record.timestamp.to_owned()),
                content: record.content.to_owned(),
                content_truncated: false,
            })
            .collect();

        matches.sort_by(|left, right| left.timestamp.cmp(&right.timestamp));
        if matches!(request.order, Some(ResultOrder::NewestFirst)) {
            matches.reverse();
        }

        let truncated = matches.len() > max_results;
        matches.truncate(max_results);

        Ok(SearchLogsResponse {
            results: matches,
            truncated,
            next_cursor: None,
        })
    }

    fn validate_search_request(request: &SearchLogsRequest) -> Result<(), String> {
        if request.source_ids.is_empty() {
            return Err("source_ids must contain at least one configured source".to_owned());
        }
        if request.source_ids.len() > MAX_SOURCES {
            return Err(format!(
                "source_ids cannot contain more than {MAX_SOURCES} entries"
            ));
        }
        if request.keyword.is_empty() {
            return Err("keyword must not be empty".to_owned());
        }
        if request.keyword.chars().count() > MAX_KEYWORD_CHARS {
            return Err(format!(
                "keyword cannot contain more than {MAX_KEYWORD_CHARS} characters"
            ));
        }
        let max_results = request.max_results.unwrap_or(50);
        if max_results == 0 || max_results > MAX_RESULTS {
            return Err(format!("max_results must be between 1 and {MAX_RESULTS}"));
        }

        let known_sources: HashSet<String> = Self::list_sources_response()
            .sources
            .into_iter()
            .map(|source| source.source_id)
            .collect();
        if let Some(unknown) = request
            .source_ids
            .iter()
            .find(|source_id| !known_sources.contains(source_id.as_str()))
        {
            return Err(format!("unknown log source: {unknown}"));
        }

        Ok(())
    }

    fn context_response(request: GetLogContextRequest) -> Result<GetLogContextResponse, String> {
        if request.before_lines > MAX_CONTEXT_LINES_PER_SIDE
            || request.after_lines > MAX_CONTEXT_LINES_PER_SIDE
        {
            return Err(format!(
                "before_lines and after_lines cannot exceed {MAX_CONTEXT_LINES_PER_SIDE}"
            ));
        }

        if request.match_ref != "match-payment-4" {
            return Err("unknown or expired match_ref".to_owned());
        }

        let all_lines = [
            ContextLine {
                line_number: 1,
                content: "INFO traceId=abc123 request received".to_owned(),
            },
            ContextLine {
                line_number: 2,
                content: "INFO traceId=abc123 orderId=10001 calling payment channel".to_owned(),
            },
            ContextLine {
                line_number: 3,
                content: "WARN traceId=abc123 channel response status=403".to_owned(),
            },
            ContextLine {
                line_number: 4,
                content:
                    "ERROR traceId=abc123 orderId=10001 PaymentAuthException: channel returned 403"
                        .to_owned(),
            },
            ContextLine {
                line_number: 5,
                content: "    at payment::authorize(payment.rs:42)".to_owned(),
            },
            ContextLine {
                line_number: 6,
                content: "Caused by: HttpException: forbidden".to_owned(),
            },
        ];

        let match_index = 3usize;
        let start_index = match_index.saturating_sub(request.before_lines);
        let end_index = (match_index + request.after_lines + 1).min(all_lines.len());
        let lines = all_lines[start_index..end_index].to_vec();
        let start_line = lines.first().map_or(4, |line| line.line_number);
        let end_line = lines.last().map_or(4, |line| line.line_number);

        Ok(GetLogContextResponse {
            source_id: "payment-test".to_owned(),
            file_id: "file-payment-application".to_owned(),
            file_name: "application.log".to_owned(),
            start_line,
            end_line,
            lines,
            truncated: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(keyword: &str) -> SearchLogsRequest {
        SearchLogsRequest {
            source_ids: vec!["payment-test".to_owned(), "order-test".to_owned()],
            keyword: keyword.to_owned(),
            case_sensitive: None,
            start_time: None,
            end_time: None,
            order: None,
            max_results: None,
            cursor: None,
        }
    }

    #[test]
    fn lists_sources_without_paths() {
        let response = LogQueryServer::list_sources_response();
        assert_eq!(response.sources.len(), 2);
        assert_eq!(response.sources[0].source_id, "payment-test");
    }

    #[test]
    fn searches_case_insensitively_by_default() {
        let response = LogQueryServer::search_response(request("paymentauthexception"))
            .expect("search should succeed");
        assert_eq!(response.results.len(), 1);
        assert_eq!(response.results[0].match_ref, "match-payment-4");
    }

    #[test]
    fn rejects_unknown_source() {
        let mut search_request = request("abc123");
        search_request.source_ids = vec!["/etc".to_owned()];
        let error = LogQueryServer::search_response(search_request)
            .expect_err("unknown source should fail");
        assert!(error.contains("unknown log source"));
    }

    #[test]
    fn returns_limited_context() {
        let response = LogQueryServer::context_response(GetLogContextRequest {
            match_ref: "match-payment-4".to_owned(),
            before_lines: 1,
            after_lines: 2,
        })
        .expect("context should succeed");

        assert_eq!(response.start_line, 3);
        assert_eq!(response.end_line, 6);
        assert_eq!(response.lines.len(), 4);
    }
}
