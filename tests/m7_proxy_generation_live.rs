#![cfg(target_os = "linux")]

use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Arc,
};

use log_query_mcp::{
    AppConfigV2, SourceRegistry, StatefulContextRequest, StatefulContextService,
    StatefulQueryRequest, StatefulQueryService,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn port(name: &str) -> u16 {
    required(name)
        .parse()
        .unwrap_or_else(|_| panic!("invalid port in {name}"))
}

fn proxy_connection(connection_id: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port("M7_GENERATION_SSH_PORT"),
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M7_GENERATION_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": required("M7_GENERATION_KNOWN_HOSTS")
        },
        "proxy": {
            "type": "command",
            "program": required("M7_GENERATION_PROXY_PROGRAM"),
            "args": ["{host}", "{port}"]
        },
        "connect_timeout_millis": 3000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 5
    })
}

fn remote_source(source_id: &str, connection_id: &str, file: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": connection_id
        },
        "root": required("M7_GENERATION_REMOTE_DIR"),
        "files": [file],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn config(cache_root: &Path) -> AppConfigV2 {
    let document = json!({
        "version": 2,
        "connections": [
            proxy_connection("proxy-generation-a"),
            proxy_connection("proxy-generation-b")
        ],
        "sources": [
            remote_source("proxy-generation-a", "proxy-generation-a", "proxy-a.log"),
            remote_source("proxy-generation-b", "proxy-generation-b", "proxy-b.log")
        ],
        "cache": {
            "root": cache_root,
            "max_bytes": 16 * 1024 * 1024,
            "max_bytes_per_source": 4 * 1024 * 1024,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 2 * 1024 * 1024,
            "max_remote_files_per_source": 20
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("valid M7 generation config")
}

fn service(config: AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config).expect("M7 generation registry");
    StatefulQueryService::new(Arc::new(registry)).expect("M7 generation query service")
}

fn contents(page: &log_query_mcp::StatefulQueryPage) -> Vec<&str> {
    page.results
        .iter()
        .map(|result| result.content.as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the M7 ProxyCommand generation-consistency fixture"]
async fn proxy_cursor_and_match_refs_remain_bound_to_their_original_generations() {
    let remote_a = required("M7_GENERATION_LOCAL_A_FILE");
    let remote_b = required("M7_GENERATION_LOCAL_B_FILE");
    let known_hosts = required("M7_GENERATION_KNOWN_HOSTS");

    fs::write(
        &remote_a,
        concat!(
            "M7CURSOR one\n",
            "M7CURSOR two\n",
            "M7KEEP source-a-old-generation\n"
        ),
    )
    .expect("reset proxy source A fixture");
    fs::write(
        &remote_b,
        concat!("M7KEEP source-b-stable\n", "M7B source-b-only\n"),
    )
    .expect("reset proxy source B fixture");

    let cache = TempDir::new().expect("M7 generation cache");
    let query = service(config(cache.path()));

    // Cursor candidates must stay pinned to the first query generation. A remote append after
    // page one must not appear through that cursor, while a fresh query must see the new data.
    let first = query
        .search(
            StatefulQueryRequest::new(vec!["proxy-generation-a".to_owned()], "M7CURSOR")
                .with_max_results(1),
        )
        .await
        .expect("first ProxyCommand cursor page");
    assert_eq!(contents(&first), vec!["M7CURSOR one"]);
    assert_eq!(first.results[0].source_id, "proxy-generation-a");
    let cursor = first.next_cursor.clone().expect("ProxyCommand cursor");

    let mut append = OpenOptions::new()
        .append(true)
        .open(&remote_a)
        .expect("open source A for append");
    append
        .write_all(b"M7CURSOR appended-after-cursor\n")
        .expect("append source A");
    append.sync_all().expect("sync source A append");
    drop(append);

    let second = query
        .search(
            StatefulQueryRequest::new(vec!["proxy-generation-a".to_owned()], "M7CURSOR")
                .with_max_results(1)
                .with_cursor(cursor),
        )
        .await
        .expect("ProxyCommand cursor must reuse original snapshot candidates");
    assert_eq!(contents(&second), vec!["M7CURSOR two"]);
    assert_eq!(second.results[0].source_id, "proxy-generation-a");
    assert!(second.next_cursor.is_none());

    let refreshed = query
        .search(
            StatefulQueryRequest::new(vec!["proxy-generation-a".to_owned()], "M7CURSOR")
                .with_max_results(10),
        )
        .await
        .expect("fresh ProxyCommand query must refresh generation");
    assert_eq!(
        contents(&refreshed),
        vec![
            "M7CURSOR one",
            "M7CURSOR two",
            "M7CURSOR appended-after-cursor",
        ]
    );
    assert!(
        refreshed
            .results
            .iter()
            .all(|result| result.source_id == "proxy-generation-a")
    );

    // Keep references for two different Proxy sources. Source A will then be replaced with a new
    // generation. Both references must still resolve to their own source + pinned generation.
    let keep_a = query
        .search(StatefulQueryRequest::new(
            vec!["proxy-generation-a".to_owned()],
            "M7KEEP",
        ))
        .await
        .expect("source A old-generation match");
    let keep_b = query
        .search(StatefulQueryRequest::new(
            vec!["proxy-generation-b".to_owned()],
            "M7KEEP",
        ))
        .await
        .expect("source B stable match");
    assert_eq!(keep_a.results.len(), 1);
    assert_eq!(keep_b.results.len(), 1);
    assert_eq!(keep_a.results[0].source_id, "proxy-generation-a");
    assert_eq!(keep_b.results[0].source_id, "proxy-generation-b");
    assert_ne!(keep_a.results[0].match_ref, keep_b.results[0].match_ref);
    let match_ref_a = keep_a.results[0].match_ref.clone();
    let match_ref_b = keep_b.results[0].match_ref.clone();

    fs::write(&remote_a, "M7NEW source-a-replacement-generation\n")
        .expect("replace source A remote file");
    let replacement = query
        .search(StatefulQueryRequest::new(
            vec!["proxy-generation-a".to_owned()],
            "M7NEW",
        ))
        .await
        .expect("replacement generation through ProxyCommand");
    assert_eq!(replacement.results.len(), 1);
    assert_eq!(replacement.results[0].source_id, "proxy-generation-a");
    assert_eq!(
        replacement.results[0].content,
        "M7NEW source-a-replacement-generation"
    );

    // Disable future SSH host verification. get_context must therefore be cache-only and use the
    // generation pins held by each match_ref rather than resyncing the current remote generation.
    let disabled_known_hosts = format!("{known_hosts}.m7-generation-disabled");
    fs::rename(&known_hosts, &disabled_known_hosts).expect("disable future SSH verification");
    let context_service = StatefulContextService::from_query_service(&query)
        .expect("context service from ProxyCommand query service");

    let context_a = context_service
        .get_context(StatefulContextRequest::new(match_ref_a).with_lines(0, 0))
        .await
        .expect("source A old match_ref must resolve from its pinned generation");
    let context_b = context_service
        .get_context(StatefulContextRequest::new(match_ref_b).with_lines(0, 0))
        .await
        .expect("source B match_ref must not cross into source A");
    fs::rename(&disabled_known_hosts, &known_hosts).expect("restore known_hosts fixture");

    assert_eq!(context_a.source_id, "proxy-generation-a");
    assert_eq!(context_a.lines.len(), 1);
    assert_eq!(context_a.lines[0].content, "M7KEEP source-a-old-generation");
    assert_eq!(context_b.source_id, "proxy-generation-b");
    assert_eq!(context_b.lines.len(), 1);
    assert_eq!(context_b.lines[0].content, "M7KEEP source-b-stable");
    assert_ne!(context_a.file_id, context_b.file_id);
}
