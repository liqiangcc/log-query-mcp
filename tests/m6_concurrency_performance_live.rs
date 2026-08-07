use std::{path::Path, sync::Arc, time::Instant};

use log_query_mcp::{AppConfigV2, SourceRegistry, StatefulQueryRequest, StatefulQueryService};
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

fn password_connection(connection_id: &str, port: u16, known_hosts: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "logreader",
        "auth": {"type": "password", "secret_ref": "M2_SSH_PASSWORD"},
        "host_key": {"known_hosts_file": known_hosts},
        "connect_timeout_millis": 1000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 30
    })
}

fn private_key_connection(connection_id: &str, port: u16, known_hosts: &str, key: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "logreader_b",
        "auth": {
            "type": "private_key",
            "key_file": key,
            "passphrase_secret_ref": "M2_KEY_PASSPHRASE"
        },
        "host_key": {"known_hosts_file": known_hosts},
        "connect_timeout_millis": 1000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 30
    })
}

fn remote_source(source_id: &str, connection_id: &str, root: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "perf",
        "backend": {"type": "ssh", "connection_id": connection_id},
        "root": root,
        "files": ["multi.log"],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn config(connections: Vec<Value>, sources: Vec<Value>, cache_root: &Path) -> AppConfigV2 {
    let value = json!({
        "version": 2,
        "connections": connections,
        "sources": sources,
        "cache": {
            "root": cache_root,
            "max_bytes": 16 * 1024 * 1024,
            "max_bytes_per_source": 8 * 1024 * 1024,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 4,
            "max_sync_bytes_per_query": 2 * 1024 * 1024,
            "max_remote_files_per_source": 20
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid concurrency config")
}

fn service(config: AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config).expect("concurrency registry");
    StatefulQueryService::new(Arc::new(registry)).expect("concurrency service")
}

fn request(source_id: &str) -> StatefulQueryRequest {
    StatefulQueryRequest::new(vec![source_id.to_owned()], "MIXED3").with_max_results(10)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the two-OpenSSH fixture from ssh-research.yml"]
async fn single_server_concurrent_queries_record_baseline() {
    let cache = TempDir::new().expect("single-server concurrency cache");
    let query = service(config(
        vec![password_connection(
            "server-a",
            port("M2_SSH_PORT"),
            &required("M2_KNOWN_HOSTS"),
        )],
        vec![remote_source(
            "remote-a",
            "server-a",
            &required("M2_REMOTE_DIR"),
        )],
        cache.path(),
    ));

    query
        .search(request("remote-a"))
        .await
        .expect("warm remote-a cache");

    let started = Instant::now();
    let (a, b, c, d) = tokio::join!(
        query.search(request("remote-a")),
        query.search(request("remote-a")),
        query.search(request("remote-a")),
        query.search(request("remote-a")),
    );
    let elapsed_ms = started.elapsed().as_millis();

    for result in [a, b, c, d] {
        let result = result.expect("concurrent single-server query");
        assert_eq!(result.results.len(), 1);
        assert_eq!(result.results[0].content, "MIXED3 server-a");
    }

    println!(
        "M6_CONCURRENCY_METRIC {{\"scenario\":\"single_server_4_queries\",\"queries\":4,\"elapsed_ms\":{elapsed_ms}}}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the two-OpenSSH fixture from ssh-research.yml"]
async fn dual_server_concurrent_queries_record_baseline() {
    let cache = TempDir::new().expect("dual-server concurrency cache");
    let query = service(config(
        vec![
            password_connection(
                "server-a",
                port("M2_SSH_PORT"),
                &required("M2_KNOWN_HOSTS"),
            ),
            private_key_connection(
                "server-b",
                port("M6_SSH_B_PORT"),
                &required("M6_SSH_B_KNOWN_HOSTS"),
                &required("M2_SSH_KEY_FILE"),
            ),
        ],
        vec![
            remote_source("remote-a", "server-a", &required("M2_REMOTE_DIR")),
            remote_source("remote-b", "server-b", &required("M6_REMOTE_B_DIR")),
        ],
        cache.path(),
    ));

    query
        .search(request("remote-a"))
        .await
        .expect("warm remote-a cache");
    query
        .search(request("remote-b"))
        .await
        .expect("warm remote-b cache");

    let started = Instant::now();
    let (server_a, server_b) = tokio::join!(
        query.search(request("remote-a")),
        query.search(request("remote-b")),
    );
    let elapsed_ms = started.elapsed().as_millis();

    let server_a = server_a.expect("concurrent server-a query");
    let server_b = server_b.expect("concurrent server-b query");
    assert_eq!(server_a.results.len(), 1);
    assert_eq!(server_b.results.len(), 1);
    assert_eq!(server_a.results[0].content, "MIXED3 server-a");
    assert_eq!(server_b.results[0].content, "MIXED3 server-b");

    println!(
        "M6_CONCURRENCY_METRIC {{\"scenario\":\"dual_server_2_queries\",\"queries\":2,\"elapsed_ms\":{elapsed_ms}}}"
    );
}
