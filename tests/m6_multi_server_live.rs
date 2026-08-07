use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use log_query_mcp::{
    AppConfigV2, SourceRegistry, StatefulQueryRequest, StatefulQueryService, ToolError,
    ToolErrorCode,
    transport::{SshConnectionManager, SshTransportError},
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

fn password_connection(connection_id: &str, port: u16, known_hosts: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M2_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "connect_timeout_millis": 500,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 30
    })
}

fn private_key_connection(
    connection_id: &str,
    port: u16,
    known_hosts: &str,
    key_file: &str,
) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "logreader_b",
        "auth": {
            "type": "private_key",
            "key_file": key_file,
            "passphrase_secret_ref": "M2_KEY_PASSPHRASE"
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "connect_timeout_millis": 500,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 30
    })
}

fn remote_source(source_id: &str, connection_id: &str, root: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": connection_id
        },
        "root": root,
        "files": ["multi.log"],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn local_source(root: &Path) -> Value {
    json!({
        "source_id": "local-multi",
        "name": "local-multi",
        "service": "local-multi",
        "environment": "test",
        "backend": {"type": "local"},
        "root": root,
        "files": ["multi.log"]
    })
}

fn config(
    connections: Vec<Value>,
    sources: Vec<Value>,
    cache_root: &Path,
    max_connections: usize,
) -> AppConfigV2 {
    let value = json!({
        "version": 2,
        "connections": connections,
        "sources": sources,
        "cache": {
            "root": cache_root,
            "max_bytes": 16 * 1024 * 1024,
            "max_bytes_per_source": 4 * 1024 * 1024,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": max_connections,
            "max_sync_bytes_per_query": 2 * 1024 * 1024,
            "max_remote_files_per_source": 20
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid M6 multi-server config")
}

fn live_connections() -> Vec<Value> {
    vec![
        password_connection("server-a", port("M2_SSH_PORT"), &required("M2_KNOWN_HOSTS")),
        private_key_connection(
            "server-b",
            port("M6_SSH_B_PORT"),
            &required("M6_SSH_B_KNOWN_HOSTS"),
            &required("M2_SSH_KEY_FILE"),
        ),
    ]
}

fn live_sources() -> Vec<Value> {
    vec![
        remote_source("remote-a", "server-a", &required("M2_REMOTE_DIR")),
        remote_source("remote-b", "server-b", &required("M6_REMOTE_B_DIR")),
    ]
}

fn service(config: AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config).expect("M6 registry");
    StatefulQueryService::new(Arc::new(registry)).expect("M6 query service")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the two-OpenSSH fixture from ssh-research.yml"]
async fn one_local_mcp_queries_two_independent_servers_and_local_source() {
    let cache = TempDir::new().expect("multi-server cache");
    let local = TempDir::new().expect("local source");
    fs::write(local.path().join("multi.log"), "MIXED3 local\n").expect("local fixture");

    let mut sources = live_sources();
    sources.push(local_source(local.path()));
    let query = service(config(live_connections(), sources, cache.path(), 2));

    let dual = query
        .search(
            StatefulQueryRequest::new(
                vec!["remote-a".to_owned(), "remote-b".to_owned()],
                "DUAL",
            )
            .with_max_results(10),
        )
        .await
        .expect("two-server query");
    assert_eq!(dual.results.len(), 2);
    assert_eq!(
        dual.results
            .iter()
            .map(|result| result.source_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["remote-a", "remote-b"])
    );
    assert_ne!(dual.results[0].file_id, dual.results[1].file_id);
    assert!(dual.results.iter().any(|result| result.content == "DUAL server-a"));
    assert!(dual.results.iter().any(|result| result.content == "DUAL server-b"));

    let mixed = query
        .search(
            StatefulQueryRequest::new(
                vec![
                    "local-multi".to_owned(),
                    "remote-a".to_owned(),
                    "remote-b".to_owned(),
                ],
                "MIXED3",
            )
            .with_max_results(10),
        )
        .await
        .expect("Local + A + B query");
    assert_eq!(mixed.results.len(), 3);
    let content = mixed
        .results
        .iter()
        .map(|result| result.content.as_str())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        content,
        BTreeSet::from(["MIXED3 local", "MIXED3 server-a", "MIXED3 server-b"])
    );
}

#[tokio::test]
#[ignore = "requires the two-OpenSSH fixture from ssh-research.yml"]
async fn global_ssh_semaphore_is_shared_across_independent_connections() {
    let cache = TempDir::new().expect("semaphore cache");
    let config = config(live_connections(), live_sources(), cache.path(), 1);
    let manager = SshConnectionManager::from_config(&config).expect("connection manager");

    let reader_a = manager
        .open_reader("server-a")
        .await
        .expect("Server A should acquire the single global permit");
    assert_eq!(
        manager
            .open_reader("server-b")
            .await
            .expect_err("Server B must share the same global connection limit"),
        SshTransportError::ConnectionLimit
    );
    reader_a.close().await.expect("close Server A reader");

    let reader_b = manager
        .open_reader("server-b")
        .await
        .expect("Server B should connect after the global permit is released");
    let bytes = reader_b
        .read_range(
            &format!("{}/multi.log", required("M6_REMOTE_B_DIR")),
            0,
            4,
        )
        .await
        .expect("read Server B after permit release");
    assert_eq!(&bytes, b"DUAL");
}

#[tokio::test]
#[ignore = "requires the two-OpenSSH fixture from ssh-research.yml"]
async fn unavailable_server_a_does_not_prevent_server_b_from_remaining_queryable() {
    let cache = TempDir::new().expect("isolation cache");
    let unavailable_port = port("M6_UNUSED_SSH_PORT");
    let connections = vec![
        password_connection(
            "server-a-unavailable",
            unavailable_port,
            &required("M2_KNOWN_HOSTS"),
        ),
        private_key_connection(
            "server-b",
            port("M6_SSH_B_PORT"),
            &required("M6_SSH_B_KNOWN_HOSTS"),
            &required("M2_SSH_KEY_FILE"),
        ),
    ];
    let sources = vec![
        remote_source(
            "remote-a-unavailable",
            "server-a-unavailable",
            &required("M2_REMOTE_DIR"),
        ),
        remote_source("remote-b", "server-b", &required("M6_REMOTE_B_DIR")),
    ];
    let query = service(config(connections, sources, cache.path(), 2));

    let error = query
        .search(StatefulQueryRequest::new(
            vec!["remote-a-unavailable".to_owned()],
            "DUAL",
        ))
        .await
        .expect_err("unavailable Server A must fail explicitly");
    assert_eq!(ToolError::from(error).code, ToolErrorCode::RemoteUnavailable);

    let server_b = query
        .search(StatefulQueryRequest::new(
            vec!["remote-b".to_owned()],
            "DUAL",
        ))
        .await
        .expect("Server B must remain independently queryable");
    assert_eq!(server_b.results.len(), 1);
    assert_eq!(server_b.results[0].content, "DUAL server-b");
}
