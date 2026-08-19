#![cfg(target_os = "linux")]

use std::{io::Read, path::Path, sync::Arc};

use log_query_mcp::{
    AppConfigV2, CacheStore, SourceRegistry, StatefulQueryRequest, StatefulQueryService, ToolError,
    ToolErrorCode,
};
use serde_json::json;

const SOURCE_ID: &str = "m7-proxy-restart";
const REMOTE_FILE: &str = "proxy-restart.log";

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn config() -> AppConfigV2 {
    let port: u16 = required("M7_RESTART_SSH_PORT")
        .parse()
        .expect("M7 restart SSH port");
    let known_hosts = required("M7_RESTART_KNOWN_HOSTS");
    let remote_dir = required("M7_RESTART_REMOTE_DIR");
    let cache_root = required("M7_RESTART_CACHE_ROOT");
    let proxy_program = required("M7_RESTART_PROXY_PROGRAM");

    let value = json!({
        "version": 2,
        "connections": [{
            "connection_id": "m7-proxy-restart-server",
            "type": "ssh",
            "host": "127.0.0.1",
            "port": port,
            "username": "logreader",
            "auth": {
                "type": "password",
                "secret_ref": "M7_RESTART_SSH_PASSWORD"
            },
            "host_key": {
                "known_hosts_file": known_hosts
            },
            "proxy": {
                "type": "command",
                "program": proxy_program,
                "args": ["{host}", "{port}"]
            },
            "connect_timeout_millis": 1000,
            "operation_timeout_millis": 5000,
            "keepalive_seconds": 5
        }],
        "sources": [{
            "source_id": SOURCE_ID,
            "name": "M7 ProxyCommand restart source",
            "service": "m7-proxy-restart",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": "m7-proxy-restart-server"
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

    AppConfigV2::from_json_str(&value.to_string()).expect("valid M7 ProxyCommand restart config")
}

fn service(config: &AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config.clone()).expect("M7 restart registry");
    StatefulQueryService::new(Arc::new(registry)).expect("M7 restart query service")
}

fn cached_text(config: &AppConfigV2) -> String {
    let cache = CacheStore::from_config(config.cache.as_ref().expect("cache config"))
        .expect("reopen M7 restart cache");
    let mut generation = cache
        .pin_current_generation(SOURCE_ID, REMOTE_FILE)
        .expect("pin current M7 restart generation");
    let mut text = String::new();
    generation
        .read_to_string(&mut text)
        .expect("read M7 restart generation");
    text
}

#[tokio::test]
#[ignore = "phase 1 of the M7 ProxyCommand restart workflow fixture"]
async fn bootstrap_cache_through_proxy_before_server_restart() {
    let config = config();
    let page = service(&config)
        .search(StatefulQueryRequest::new(
            vec![SOURCE_ID.to_owned()],
            "M7_RESTART_BASE",
        ))
        .await
        .expect("initial ProxyCommand query before server restart");

    assert_eq!(page.results.len(), 1);
    assert_eq!(page.results[0].content, "M7_RESTART_BASE before-restart");
    assert_eq!(cached_text(&config), "M7_RESTART_BASE before-restart\n");
}

#[tokio::test]
#[ignore = "phase 2 of the M7 ProxyCommand restart workflow fixture"]
async fn proxy_server_outage_fails_closed_without_serving_stale_cache() {
    let config = config();
    let error = service(&config)
        .search(StatefulQueryRequest::new(
            vec![SOURCE_ID.to_owned()],
            "M7_RESTART_BASE",
        ))
        .await
        .expect_err("on-query refresh through ProxyCommand must fail while sshd is unavailable");

    assert_eq!(
        ToolError::from(error).code,
        ToolErrorCode::RemoteUnavailable
    );

    // The last valid cache remains recoverable internally, but allow_stale_on_error=false
    // means it must never be returned as a successful query result during the outage.
    assert_eq!(cached_text(&config), "M7_RESTART_BASE before-restart\n");
}

#[tokio::test]
#[ignore = "phase 3 of the M7 ProxyCommand restart workflow fixture"]
async fn proxy_query_recovers_after_server_restart_and_append() {
    let config = config();
    let page = service(&config)
        .search(
            StatefulQueryRequest::new(vec![SOURCE_ID.to_owned()], "M7_RESTART")
                .with_max_results(10),
        )
        .await
        .expect("ProxyCommand query should recover after sshd restart");

    assert_eq!(page.results.len(), 2);
    assert_eq!(page.results[0].content, "M7_RESTART_BASE before-restart");
    assert_eq!(page.results[1].content, "M7_RESTART_AFTER after-restart");
    assert_eq!(
        cached_text(&config),
        "M7_RESTART_BASE before-restart\nM7_RESTART_AFTER after-restart\n"
    );
}
