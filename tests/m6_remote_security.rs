use std::sync::Arc;

use log_query_mcp::{
    AppConfigV2, GetLogContextRequest, ListLogSourcesRequest, ListLogSourcesResponse,
    LogQueryMcpServer, SearchLogsRequest, SourceRegistry, SourceRegistryError, ToolError,
    ToolErrorCode, transport::SshTransportError,
};
use rmcp::{ServerHandler, model::RawContent};
use serde_json::json;
use tempfile::TempDir;

const PRIVATE_HOST: &str = "m6-private-host.invalid";
const PRIVATE_USERNAME: &str = "m6-private-user";
const PRIVATE_CONNECTION_ID: &str = "m6-private-connection";
const PRIVATE_SECRET_REF: &str = "M6_PRIVATE_PASSWORD";
const PRIVATE_REMOTE_ROOT: &str = "/srv/private/logs";

#[test]
fn ai_facing_requests_reject_remote_connection_and_path_control_fields() {
    for (field, value) in [
        ("host", json!(PRIVATE_HOST)),
        ("port", json!(2222)),
        ("username", json!(PRIVATE_USERNAME)),
        ("password", json!("do-not-accept-passwords-from-ai")),
        ("secret_ref", json!(PRIVATE_SECRET_REF)),
        ("connection_id", json!(PRIVATE_CONNECTION_ID)),
        ("root", json!(PRIVATE_REMOTE_ROOT)),
        ("path", json!("../../etc/passwd")),
        ("remote_path", json!("/etc/passwd")),
    ] {
        let mut search = json!({
            "source_ids": ["remote-security"],
            "keyword": "needle"
        });
        search
            .as_object_mut()
            .expect("search request object")
            .insert(field.to_owned(), value.clone());
        assert!(
            serde_json::from_value::<SearchLogsRequest>(search).is_err(),
            "search_logs must reject client-controlled field {field}"
        );

        let mut context = json!({
            "match_ref": "mref_0123456789abcdef0123456789abcdef"
        });
        context
            .as_object_mut()
            .expect("context request object")
            .insert(field.to_owned(), value.clone());
        assert!(
            serde_json::from_value::<GetLogContextRequest>(context).is_err(),
            "get_log_context must reject client-controlled field {field}"
        );

        let mut list = json!({});
        list.as_object_mut()
            .expect("list request object")
            .insert(field.to_owned(), value);
        assert!(
            serde_json::from_value::<ListLogSourcesRequest>(list).is_err(),
            "list_log_sources must reject client-controlled field {field}"
        );
    }
}

#[test]
fn mcp_tool_surface_contains_only_read_only_log_semantics() {
    let (_fixture, server) = remote_server();

    for allowed in ["list_log_sources", "search_logs", "get_log_context"] {
        assert!(server.get_tool(allowed).is_some(), "missing tool {allowed}");
    }

    for forbidden in [
        "ssh_exec",
        "exec",
        "shell",
        "run_command",
        "read_remote_file",
        "write_remote_file",
        "upload_file",
        "delete_file",
        "restart_service",
    ] {
        assert!(
            server.get_tool(forbidden).is_none(),
            "forbidden capability unexpectedly exposed as MCP tool: {forbidden}"
        );
    }
}

#[test]
fn list_log_sources_does_not_disclose_remote_connection_or_secret_metadata() {
    let (fixture, server) = remote_server();
    let result = server.list_log_sources_result();
    assert_eq!(result.is_error, Some(false));
    let text = first_text(&result);
    let response: ListLogSourcesResponse =
        serde_json::from_str(text).expect("list_log_sources JSON should parse");

    assert_eq!(response.sources.len(), 1);
    assert_eq!(response.sources[0].source_id, "remote-security");
    assert_eq!(response.sources[0].service, "security-service");

    let cache_path = fixture.path().join("cache").to_string_lossy().into_owned();
    for sensitive in [
        PRIVATE_HOST,
        PRIVATE_USERNAME,
        PRIVATE_CONNECTION_ID,
        PRIVATE_SECRET_REF,
        PRIVATE_REMOTE_ROOT,
        cache_path.as_str(),
        "known_hosts",
        "password",
    ] {
        assert!(
            !text.contains(sensitive),
            "list_log_sources leaked sensitive remote metadata: {sensitive}"
        );
    }
}

#[test]
fn remote_tool_errors_are_code_only_and_do_not_render_nested_remote_details() {
    let source_id = "remote-source-id-that-must-not-leak";

    for (source, expected) in [
        (
            SshTransportError::AuthenticationFailed,
            ToolErrorCode::RemoteAuthFailed,
        ),
        (
            SshTransportError::HostKeyVerificationFailed,
            ToolErrorCode::HostKeyVerificationFailed,
        ),
        (
            SshTransportError::SecretUnavailable,
            ToolErrorCode::InternalError,
        ),
    ] {
        let error = ToolError::from(SourceRegistryError::RemoteTransport {
            source_id: source_id.to_owned(),
            source,
        });
        assert_eq!(error.code, expected);
        let json = error.to_json_string().expect("tool error should serialize");
        assert!(!json.contains(source_id));
        assert!(!json.contains(PRIVATE_SECRET_REF));
        assert!(!json.contains(PRIVATE_HOST));
        assert!(!json.contains(PRIVATE_USERNAME));
        assert!(!json.contains("details"));
        assert!(!json.contains("cause"));
        assert!(!json.contains("backtrace"));
    }
}

fn remote_server() -> (TempDir, LogQueryMcpServer) {
    let fixture = TempDir::new().expect("security fixture tempdir");
    let known_hosts = fixture.path().join("known_hosts");
    let cache_root = fixture.path().join("cache");

    let config = json!({
        "version": 2,
        "connections": [{
            "connection_id": PRIVATE_CONNECTION_ID,
            "type": "ssh",
            "host": PRIVATE_HOST,
            "port": 2222,
            "username": PRIVATE_USERNAME,
            "auth": {
                "type": "password",
                "secret_ref": PRIVATE_SECRET_REF
            },
            "host_key": {
                "known_hosts_file": known_hosts
            },
            "connect_timeout_millis": 1000,
            "operation_timeout_millis": 1000,
            "keepalive_seconds": 30
        }],
        "sources": [{
            "source_id": "remote-security",
            "name": "Remote security source",
            "description": "safe public description",
            "service": "security-service",
            "environment": "test",
            "tags": ["remote", "security"],
            "backend": {
                "type": "ssh",
                "connection_id": PRIVATE_CONNECTION_ID
            },
            "root": PRIVATE_REMOTE_ROOT,
            "files": ["application.log"],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": cache_root,
            "max_bytes": 1048576,
            "max_bytes_per_source": 524288,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 262144,
            "max_remote_files_per_source": 10
        }
    });

    let config = AppConfigV2::from_json_str(&config.to_string()).expect("valid v2 security config");
    let registry = SourceRegistry::from_config_v2(config).expect("remote registry should build");
    let server = LogQueryMcpServer::from_registry(Arc::new(registry)).expect("MCP server should build");
    (fixture, server)
}

fn first_text(result: &rmcp::model::CallToolResult) -> &str {
    assert_eq!(result.content.len(), 1);
    match &result.content[0].raw {
        RawContent::Text(text) => &text.text,
        other => panic!("expected text content, got {other:?}"),
    }
}
