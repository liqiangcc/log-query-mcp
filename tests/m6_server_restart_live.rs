use std::{io::Read, path::Path, sync::Arc};

use log_query_mcp::{
    AppConfigV2, CacheStore, SourceRegistry, StatefulQueryRequest, StatefulQueryService, ToolError,
    ToolErrorCode,
};
use serde_json::json;

const SOURCE_ID: &str = "restart-live";
const REMOTE_FILE: &str = "restart-live.log";

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn config() -> AppConfigV2 {
    let port: u16 = required("M2_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let remote_dir = required("M2_REMOTE_DIR");
    let cache_root = required("M6_RESTART_CACHE_ROOT");
    let value = json!({
        "version": 2,
        "connections": [{
            "connection_id": "restart-server",
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
        }],
        "sources": [{
            "source_id": SOURCE_ID,
            "name": "Restart live source",
            "service": "restart-live",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": "restart-server"
            },
            "root": remote_dir,
            "files": [REMOTE_FILE],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": Path::new(&cache_root),
            "max_bytes": 4 * 1024 * 1024,
            "max_bytes_per_source": 2 * 1024 * 1024,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 1,
            "max_sync_bytes_per_query": 1024 * 1024,
            "max_remote_files_per_source": 10
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid restart config")
}

fn service(config: &AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config.clone()).expect("restart registry");
    StatefulQueryService::new(Arc::new(registry)).expect("restart query service")
}

#[tokio::test]
#[ignore = "phase 1 of the OpenSSH restart fixture from ssh-research.yml"]
async fn bootstrap_cache_before_server_restart() {
    let config = config();
    let page = service(&config)
        .search(StatefulQueryRequest::new(
            vec![SOURCE_ID.to_owned()],
            "RESTART_BASE",
        ))
        .await
        .expect("initial query before server restart");
    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].content, "RESTART_BASE before-restart");

    let cache = CacheStore::from_config(config.cache.as_ref().expect("cache config"))
        .expect("reopen cache after bootstrap");
    let mut generation = cache
        .pin_current_generation(SOURCE_ID, REMOTE_FILE)
        .expect("pin bootstrapped generation");
    let mut text = String::new();
    generation
        .read_to_string(&mut text)
        .expect("read bootstrapped generation");
    assert_eq!(text, "RESTART_BASE before-restart\n");
}

#[tokio::test]
#[ignore = "phase 2 of the OpenSSH restart fixture from ssh-research.yml"]
async fn server_outage_fails_closed_without_destroying_last_valid_cache() {
    let config = config();
    let error = service(&config)
        .search(StatefulQueryRequest::new(
            vec![SOURCE_ID.to_owned()],
            "RESTART_BASE",
        ))
        .await
        .expect_err("on-query refresh must fail while sshd is unavailable");
    assert_eq!(
        ToolError::from(error).code,
        ToolErrorCode::RemoteUnavailable
    );

    let cache = CacheStore::from_config(config.cache.as_ref().expect("cache config"))
        .expect("reopen cache while remote is unavailable");
    let mut generation = cache
        .pin_current_generation(SOURCE_ID, REMOTE_FILE)
        .expect("last valid generation must remain available locally");
    let mut text = String::new();
    generation
        .read_to_string(&mut text)
        .expect("read preserved generation");
    assert_eq!(text, "RESTART_BASE before-restart\n");
}

#[tokio::test]
#[ignore = "phase 3 of the OpenSSH restart fixture from ssh-research.yml"]
async fn query_recovers_after_server_restart_and_incremental_append() {
    let config = config();
    let page = service(&config)
        .search(
            StatefulQueryRequest::new(vec![SOURCE_ID.to_owned()], "RESTART").with_max_results(10),
        )
        .await
        .expect("query should recover after sshd restart");
    assert_eq!(page.results.len(), 2);
    assert_eq!(page.results[0].content, "RESTART_BASE before-restart");
    assert_eq!(page.results[1].content, "RESTART_AFTER after-restart");

    let cache = CacheStore::from_config(config.cache.as_ref().expect("cache config"))
        .expect("reopen cache after recovery");
    let mut generation = cache
        .pin_current_generation(SOURCE_ID, REMOTE_FILE)
        .expect("pin recovered generation");
    let mut text = String::new();
    generation
        .read_to_string(&mut text)
        .expect("read recovered generation");
    assert_eq!(
        text,
        "RESTART_BASE before-restart\nRESTART_AFTER after-restart\n"
    );
}
