use std::{path::Path, sync::Arc};

use log_query_mcp::{
    AppConfigV2, SourceRegistry, SourceRegistryError, StatefulQueryError, StatefulQueryRequest,
    StatefulQueryService, ToolError, ToolErrorCode,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn config(port: u16, known_hosts: &str, cache_root: &Path, source: Value) -> AppConfigV2 {
    let value = json!({
        "version": 2,
        "connections": [{
            "connection_id": "m6-security",
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
            "connect_timeout_millis": 5000,
            "operation_timeout_millis": 5000,
            "keepalive_seconds": 30
        }],
        "sources": [source],
        "cache": {
            "root": cache_root,
            "max_bytes": 10485760,
            "max_bytes_per_source": 5242880,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 1048576,
            "max_remote_files_per_source": 20
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid M6 security config")
}

fn explicit_source(root: &str) -> Value {
    json!({
        "source_id": "explicit-symlink",
        "name": "explicit symlink",
        "service": "security",
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": "m6-security"
        },
        "root": root,
        "files": ["escape.log"],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn directory_source(root: &str) -> Value {
    json!({
        "source_id": "directory-symlink",
        "name": "directory symlink",
        "service": "security",
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": "m6-security"
        },
        "root": root,
        "directories": [{
            "path": ".",
            "recursive": false,
            "include_suffixes": [".log"]
        }],
        "sync": {
            "freshness": "on_query",
            "bootstrap": {"type": "full"},
            "allow_stale_on_error": false
        }
    })
}

fn service(config: AppConfigV2) -> StatefulQueryService {
    let registry = SourceRegistry::from_config_v2(config).expect("M6 registry");
    StatefulQueryService::new(Arc::new(registry)).expect("M6 query service")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the OpenSSH/SFTP fixture from ssh-research.yml"]
async fn real_sftp_symlinks_cannot_escape_the_admin_log_boundary() {
    let port: u16 = required("M2_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let remote_dir = required("M2_REMOTE_DIR");
    let directory_root = required("M5_REMOTE_DIRECTORY_ROOT");

    // An explicitly configured symlink is not a regular file and must fail before any content
    // behind the link can be synchronized into the cache.
    let explicit_cache = TempDir::new().expect("explicit cache");
    let explicit = service(config(
        port,
        &known_hosts,
        explicit_cache.path(),
        explicit_source(&remote_dir),
    ));
    let error = explicit
        .search(StatefulQueryRequest::new(
            vec!["explicit-symlink".to_owned()],
            "root:",
        ))
        .await
        .expect_err("explicit remote symlink must be rejected");
    assert!(matches!(
        error,
        StatefulQueryError::SourceRegistry(SourceRegistryError::RemoteExplicitFileNotRegular {
            ..
        })
    ));
    assert_eq!(ToolError::from(error).code, ToolErrorCode::SyncFailed);

    // A symlink discovered inside an allowed directory must be skipped by lstat/regular-file
    // filtering. /etc/passwd contains "root:", so observing any result here would demonstrate
    // that the symlink target was followed.
    let directory_cache = TempDir::new().expect("directory cache");
    let directory = service(config(
        port,
        &known_hosts,
        directory_cache.path(),
        directory_source(&directory_root),
    ));
    let page = directory
        .search(StatefulQueryRequest::new(
            vec!["directory-symlink".to_owned()],
            "root:",
        ))
        .await
        .expect("directory discovery should safely skip symlinks");
    assert!(page.results.is_empty());
}
