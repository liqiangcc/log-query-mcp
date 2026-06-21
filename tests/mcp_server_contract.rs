use std::{fs, path::Path, sync::Arc};

use rmcp::{
    ServerHandler,
    model::{CallToolResult, RawContent},
};
use serde_json::json;
use tempfile::tempdir;

use log_query_mcp::{
    AppConfig, CONFIG_VERSION, Encoding, GetLogContextRequest, GetLogContextResponse,
    ListLogSourcesResponse, LogQueryMcpServer, LogSource, LogSourceConfig, SearchLogsRequest,
    SearchLogsResponse, SearchOrder, SourceRegistry, StatefulQueryService, TimestampRule,
    ToolErrorCode,
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_adapter_lists_searches_and_reads_context_as_contract_json() {
    let root = tempdir().expect("source root should be created");
    fs::write(
        root.path().join("application.log"),
        concat!(
            "2026-06-19T14:00:00+09:00 before\n",
            "2026-06-19T14:00:01+09:00 traceId=abc123 failed\n",
            "2026-06-19T14:00:02+09:00 after\n"
        ),
    )
    .expect("fixture should be written");
    let server = server(root.path());

    let tools = ["list_log_sources", "search_logs", "get_log_context"];
    for tool in tools {
        let tool = server.get_tool(tool).expect("tool should be registered");
        assert!(
            tool.output_schema.is_some(),
            "{} should expose an output schema",
            tool.name
        );
    }
    let search_tool = server
        .get_tool("search_logs")
        .expect("search_logs should be registered");
    assert!(
        search_tool
            .input_schema
            .get("properties")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|properties| {
                properties.contains_key("source_ids") && properties.contains_key("keyword")
            })
    );

    let sources: ListLogSourcesResponse = parse_success(server.list_log_sources_result());
    assert_eq!(
        sources,
        ListLogSourcesResponse {
            sources: vec![LogSource {
                source_id: "payment-test".to_owned(),
                name: "Payment test".to_owned(),
                description: "payment-service application logs".to_owned(),
                service: "payment-service".to_owned(),
                environment: "test".to_owned(),
                tags: vec!["payment".to_owned(), "java".to_owned()],
            }],
        }
    );

    let search: SearchLogsResponse = parse_success(
        server
            .search_logs_result(SearchLogsRequest {
                source_ids: vec!["payment-test".to_owned()],
                keyword: "traceId=abc123".to_owned(),
                case_sensitive: false,
                start_time: None,
                end_time: None,
                order: SearchOrder::OldestFirst,
                max_results: Some(10),
                cursor: None,
            })
            .await,
    );
    assert_eq!(search.results.len(), 1);
    assert_eq!(search.results[0].source_id, "payment-test");
    assert_eq!(search.results[0].file_name, "application.log");
    assert_eq!(search.results[0].line_number, 2);
    assert_eq!(
        search.results[0].timestamp.as_deref(),
        Some("2026-06-19T14:00:01+09:00")
    );
    assert!(search.results[0].content.contains("traceId=abc123"));
    assert!(!search.truncated);
    assert!(search.next_cursor.is_none());

    let context: GetLogContextResponse = parse_success(
        server
            .get_log_context_result(GetLogContextRequest {
                match_ref: search.results[0].match_ref.clone(),
                before_lines: 1,
                after_lines: 1,
            })
            .await,
    );
    assert_eq!(context.source_id, "payment-test");
    assert_eq!(context.file_name, "application.log");
    assert_eq!(context.start_line, 1);
    assert_eq!(context.end_line, 3);
    assert_eq!(context.lines.len(), 3);
    assert_eq!(context.lines[1].line_number, 2);
    assert!(context.lines[1].content.contains("traceId=abc123"));
    assert!(!context.truncated);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mcp_adapter_returns_sanitized_tool_error_json() {
    let root = tempdir().expect("source root should be created");
    fs::write(root.path().join("application.log"), "traceId=abc123\n")
        .expect("fixture should be written");
    let server = server(root.path());

    let result = server
        .search_logs_result(SearchLogsRequest {
            source_ids: vec!["missing-source".to_owned()],
            keyword: "traceId=abc123".to_owned(),
            case_sensitive: false,
            start_time: None,
            end_time: None,
            order: SearchOrder::OldestFirst,
            max_results: Some(10),
            cursor: None,
        })
        .await;

    assert_eq!(result.is_error, Some(true));
    let text = first_text(&result);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(text).expect("tool error should parse"),
        json!({
            "code": ToolErrorCode::UnknownSource.wire_code(),
            "message": ToolErrorCode::UnknownSource.default_message(),
            "retryable": false
        })
    );
    assert!(!text.contains("missing-source"));
    assert!(!text.contains("details"));
    assert!(!text.contains("path"));
    assert!(!text.contains("backtrace"));
}

#[test]
fn mcp_requests_reject_unknown_fields_and_unsupported_order() {
    let unknown_field = serde_json::from_value::<SearchLogsRequest>(json!({
        "source_ids": ["payment-test"],
        "keyword": "traceId=abc123",
        "path": "/var/log/payment/application.log"
    }));
    assert!(unknown_field.is_err());

    let unsupported_order = serde_json::from_value::<SearchLogsRequest>(json!({
        "source_ids": ["payment-test"],
        "keyword": "traceId=abc123",
        "order": "newest_first"
    }));
    assert!(unsupported_order.is_err());

    let context_with_path = serde_json::from_value::<GetLogContextRequest>(json!({
        "match_ref": "mref_0123456789abcdef0123456789abcdef",
        "line_number": 42
    }));
    assert!(context_with_path.is_err());
}

fn server(root: &Path) -> LogQueryMcpServer {
    let registry = SourceRegistry::from_config(config(root)).expect("registry should build");
    let query_service =
        StatefulQueryService::new(Arc::new(registry)).expect("query service should build");
    LogQueryMcpServer::new(query_service).expect("mcp server should build")
}

fn config(root: &Path) -> AppConfig {
    AppConfig {
        version: CONFIG_VERSION,
        sources: vec![LogSourceConfig {
            source_id: "payment-test".to_owned(),
            name: "Payment test".to_owned(),
            description: "payment-service application logs".to_owned(),
            service: "payment-service".to_owned(),
            environment: "test".to_owned(),
            tags: vec!["payment".to_owned(), "java".to_owned()],
            enabled: true,
            encoding: Encoding::Utf8,
            root: root.to_path_buf(),
            files: vec!["application.log".into()],
            directories: Vec::new(),
            timestamp_rule: Some(TimestampRule::Rfc3339 { prefix_bytes: 64 }),
        }],
        limits: Default::default(),
    }
}

fn parse_success<T>(result: CallToolResult) -> T
where
    T: serde::de::DeserializeOwned,
{
    assert_eq!(result.is_error, Some(false));
    serde_json::from_str(first_text(&result)).expect("success JSON should match model")
}

fn first_text(result: &CallToolResult) -> &str {
    assert_eq!(result.content.len(), 1);
    match &result.content[0].raw {
        RawContent::Text(text) => &text.text,
        other => panic!("expected text content, got {other:?}"),
    }
}
