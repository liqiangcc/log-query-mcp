#![cfg(target_os = "linux")]

use std::{collections::BTreeSet, fs, path::Path, sync::Arc};

use log_query_mcp::{
    AppConfigV2, SourceRegistry, StatefulQueryRequest, StatefulQueryService, ToolError,
    ToolErrorCode,
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

fn direct_connection(connection_id: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port("M7_MIXED_SSH_PORT"),
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M7_MIXED_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": required("M7_MIXED_KNOWN_HOSTS")
        },
        "connect_timeout_millis": 3000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 5
    })
}

fn proxy_connection(connection_id: &str, program: &str) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port("M7_MIXED_SSH_PORT"),
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M7_MIXED_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": required("M7_MIXED_KNOWN_HOSTS")
        },
        "proxy": {
            "type": "command",
            "program": program,
            "args": ["{host}", "{port}"]
        },
        "connect_timeout_millis": 3000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 5
    })
}

fn remote_source(source_id: &str, connection_id: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": connection_id
        },
        "root": required("M7_MIXED_REMOTE_DIR"),
        "files": ["mixed.log"],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn local_source(root: &Path) -> Value {
    json!({
        "source_id": "local-m7",
        "name": "local-m7",
        "service": "local-m7",
        "environment": "test",
        "backend": {"type": "local"},
        "root": root,
        "files": ["mixed.log"]
    })
}

fn config(local_root: &Path, cache_root: &Path, include_failed_proxy: bool) -> AppConfigV2 {
    let mut connections = vec![
        direct_connection("direct-good"),
        proxy_connection("proxy-good", &required("M7_MIXED_PROXY_PROGRAM")),
    ];
    let mut sources = vec![
        local_source(local_root),
        remote_source("direct-remote", "direct-good"),
        remote_source("proxy-remote", "proxy-good"),
    ];

    if include_failed_proxy {
        connections.push(proxy_connection("proxy-bad", "/usr/bin/false"));
        sources.push(remote_source("proxy-bad-remote", "proxy-bad"));
    }

    let document = json!({
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
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 2 * 1024 * 1024,
            "max_remote_files_per_source": 20
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("valid M7 mixed-query config")
}

fn service(config: AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config).expect("M7 mixed SourceRegistry");
    StatefulQueryService::new(Arc::new(registry)).expect("M7 mixed query service")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the M7 mixed-query OpenSSH fixture"]
async fn local_direct_and_proxy_sources_query_through_one_service() {
    let cache = TempDir::new().expect("M7 mixed cache");
    let local = TempDir::new().expect("M7 local source");
    fs::write(local.path().join("mixed.log"), "M7MIX local\n").expect("write local mixed fixture");

    let query = service(config(local.path(), cache.path(), false));
    let result = query
        .search(
            StatefulQueryRequest::new(
                vec![
                    "local-m7".to_owned(),
                    "direct-remote".to_owned(),
                    "proxy-remote".to_owned(),
                ],
                "M7MIX",
            )
            .with_max_results(10),
        )
        .await
        .expect("Local + Direct + Proxy mixed query must succeed");

    assert_eq!(result.results.len(), 3);
    assert_eq!(
        result
            .results
            .iter()
            .map(|item| item.source_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["direct-remote", "local-m7", "proxy-remote"])
    );
    assert!(
        result
            .results
            .iter()
            .any(|item| item.source_id == "local-m7" && item.content == "M7MIX local")
    );
    assert!(
        result
            .results
            .iter()
            .any(|item| item.source_id == "direct-remote" && item.content == "M7MIX remote")
    );
    assert!(
        result
            .results
            .iter()
            .any(|item| item.source_id == "proxy-remote" && item.content == "M7MIX remote")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the M7 mixed-query OpenSSH fixture"]
async fn failed_proxy_source_does_not_poison_local_direct_or_other_proxy_sources() {
    let cache = TempDir::new().expect("M7 isolation cache");
    let local = TempDir::new().expect("M7 isolation local source");
    fs::write(local.path().join("mixed.log"), "M7MIX local\n")
        .expect("write local isolation fixture");

    let query = service(config(local.path(), cache.path(), true));

    let error = query
        .search(StatefulQueryRequest::new(
            vec!["proxy-bad-remote".to_owned()],
            "M7MIX",
        ))
        .await
        .expect_err("failed ProxyCommand source must fail explicitly");
    assert_eq!(
        ToolError::from(error).code,
        ToolErrorCode::RemoteUnavailable
    );

    let healthy = query
        .search(
            StatefulQueryRequest::new(
                vec![
                    "local-m7".to_owned(),
                    "direct-remote".to_owned(),
                    "proxy-remote".to_owned(),
                ],
                "M7MIX",
            )
            .with_max_results(10),
        )
        .await
        .expect("failed ProxyCommand source must not poison healthy sources");

    assert_eq!(healthy.results.len(), 3);
    assert_eq!(
        healthy
            .results
            .iter()
            .map(|item| item.source_id.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from(["direct-remote", "local-m7", "proxy-remote"])
    );
}
