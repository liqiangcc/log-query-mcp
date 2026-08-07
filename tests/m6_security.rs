use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};

use log_query_mcp::{
    AppConfigV2, ByteRange, CacheCoverage, CacheStore, CacheStoreLimits, GenerationMetadata,
    GetLogContextRequest, ListLogSourcesRequest, LogSource, SearchLogsRequest, SourceRegistry,
    SourceRegistryError,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn remote_config(cache_root: &Path) -> Value {
    json!({
        "version": 2,
        "connections": [{
            "connection_id": "secure-ssh",
            "type": "ssh",
            "host": "127.0.0.1",
            "port": 22,
            "username": "log-reader",
            "auth": {
                "type": "password",
                "secret_ref": "LOG_QUERY_MCP_TEST_PASSWORD"
            },
            "host_key": {
                "known_hosts_file": "/tmp/log-query-mcp-known-hosts"
            },
            "connect_timeout_millis": 1000,
            "operation_timeout_millis": 1000,
            "keepalive_seconds": 30
        }],
        "sources": [{
            "source_id": "secure-remote",
            "name": "Secure Remote",
            "description": "remote source",
            "service": "orders",
            "environment": "test",
            "tags": ["security"],
            "backend": {
                "type": "ssh",
                "connection_id": "secure-ssh"
            },
            "root": "/srv/logs/orders",
            "files": ["application.log"],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
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
    })
}

#[test]
fn mcp_requests_cannot_supply_connection_credentials_or_paths() {
    let search_base = json!({
        "source_ids": ["secure-remote"],
        "keyword": "trace-123"
    });
    for (field, value) in [
        ("host", json!("attacker.example")),
        ("port", json!(22)),
        ("username", json!("root")),
        ("password", json!("plaintext")),
        ("secret_ref", json!("OTHER_SECRET")),
        ("path", json!("/etc/shadow")),
        ("remote_path", json!("../../etc/shadow")),
    ] {
        let mut request = search_base.clone();
        request[field] = value;
        assert!(
            serde_json::from_value::<SearchLogsRequest>(request).is_err(),
            "search_logs must reject unexpected field {field}"
        );
    }

    for (field, value) in [
        ("host", json!("attacker.example")),
        ("password", json!("plaintext")),
        ("path", json!("/etc/shadow")),
    ] {
        let mut request = json!({
            "match_ref": "mr_invalid",
            "before_lines": 0,
            "after_lines": 0
        });
        request[field] = value;
        assert!(
            serde_json::from_value::<GetLogContextRequest>(request).is_err(),
            "get_log_context must reject unexpected field {field}"
        );
    }

    assert!(
        serde_json::from_value::<ListLogSourcesRequest>(json!({"host": "attacker.example"}))
            .is_err()
    );
}

#[test]
fn public_log_source_response_does_not_expose_remote_connection_details() {
    let source = LogSource {
        source_id: "secure-remote".to_owned(),
        name: "Secure Remote".to_owned(),
        description: "remote source".to_owned(),
        service: "orders".to_owned(),
        environment: "test".to_owned(),
        tags: vec!["security".to_owned()],
    };
    let value = serde_json::to_value(source).expect("serialize public source");
    let object = value.as_object().expect("source response object");
    let keys = object.keys().map(String::as_str).collect::<BTreeSet<_>>();
    assert_eq!(
        keys,
        BTreeSet::from([
            "description",
            "environment",
            "name",
            "service",
            "source_id",
            "tags",
        ])
    );
    for forbidden in [
        "host",
        "port",
        "username",
        "password",
        "secret_ref",
        "key_file",
        "known_hosts_file",
        "root",
        "files",
        "directories",
        "connection_id",
    ] {
        assert!(!object.contains_key(forbidden));
    }
}

#[test]
fn v2_config_rejects_plaintext_password_and_remote_path_escape() {
    let temp = TempDir::new().expect("cache root");

    let mut plaintext = remote_config(temp.path());
    plaintext["connections"][0]["auth"]["password"] = json!("do-not-store-me");
    assert!(
        AppConfigV2::from_json_str(&plaintext.to_string()).is_err(),
        "plaintext password must not be part of the v2 auth contract"
    );

    for invalid_path in ["../escape.log", "/etc/passwd", "logs/../escape.log"] {
        let mut traversal = remote_config(temp.path());
        traversal["sources"][0]["files"] = json!([invalid_path]);
        assert!(
            AppConfigV2::from_json_str(&traversal.to_string()).is_err(),
            "remote configured file path must reject {invalid_path}"
        );
    }

    let mut directory_escape = remote_config(temp.path());
    directory_escape["sources"][0]["files"] = json!([]);
    directory_escape["sources"][0]["directories"] = json!([{
        "path": "../outside",
        "recursive": false,
        "include_suffixes": [".log"]
    }]);
    assert!(AppConfigV2::from_json_str(&directory_escape.to_string()).is_err());

    let mut stale = remote_config(temp.path());
    stale["sources"][0]["sync"]["allow_stale_on_error"] = json!(true);
    assert!(
        AppConfigV2::from_json_str(&stale.to_string()).is_err(),
        "v2 MVP must fail closed instead of silently querying stale remote cache"
    );
}

#[test]
fn remote_recursive_discovery_is_explicitly_rejected_in_mvp() {
    let temp = TempDir::new().expect("cache root");
    let mut value = remote_config(temp.path());
    value["sources"][0]["files"] = json!([]);
    value["sources"][0]["directories"] = json!([{
        "path": "archive",
        "recursive": true,
        "include_suffixes": [".log"]
    }]);
    let config = AppConfigV2::from_json_str(&value.to_string()).expect("valid v2 shape");
    assert!(matches!(
        SourceRegistry::from_config_v2(config),
        Err(SourceRegistryError::RemoteRecursiveDiscoveryUnsupported { .. })
    ));
}

#[test]
fn exported_ssh_transport_surface_remains_read_only() {
    let source = include_str!("../src/transport/ssh.rs");
    let methods = source
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix("pub async fn ")
                .and_then(|rest| rest.split('(').next())
                .map(str::to_owned)
        })
        .collect::<BTreeSet<_>>();

    for required in [
        "open_reader",
        "stat",
        "lstat",
        "read_dir",
        "read_range",
        "close",
    ] {
        assert!(
            methods.contains(required),
            "missing expected read-only API {required}"
        );
    }
    for forbidden in [
        "exec",
        "execute",
        "shell",
        "write",
        "write_file",
        "upload",
        "remove",
        "delete",
        "rename",
        "mkdir",
        "create_file",
    ] {
        assert!(
            !methods.contains(forbidden),
            "SSH transport must not expose mutation API {forbidden}"
        );
    }

    let exports = include_str!("../src/transport/mod.rs");
    assert!(exports.contains("SshReadTransport"));
    assert!(!exports.contains("SshWriteTransport"));
    assert!(!exports.contains("SshExecTransport"));
}

#[test]
fn cache_metadata_schema_contains_no_connection_or_secret_fields() {
    let temp = TempDir::new().expect("cache root");
    let store = CacheStore::open(
        temp.path(),
        CacheStoreLimits {
            max_bytes: 1024 * 1024,
            max_bytes_per_source: 1024 * 1024,
            retention: Duration::from_secs(3600),
            max_generations_per_file: 4,
        },
    )
    .expect("cache store");

    let bytes = b"ordinary log line\n";
    let mut staged = store
        .begin_generation("secure-remote", "application.log")
        .expect("stage generation");
    staged.write_all(bytes).expect("write generation");
    staged
        .commit(GenerationMetadata {
            remote_size: u64::try_from(bytes.len()).expect("length"),
            cached_range: ByteRange::new(0, u64::try_from(bytes.len()).expect("length"))
                .expect("range"),
            remote_mtime_millis: Some(1),
            continuity_fingerprint: Some("sha256:test-fingerprint".to_owned()),
            coverage: CacheCoverage::Full,
        })
        .expect("commit generation");

    let forbidden_keys = BTreeSet::from([
        "host",
        "port",
        "username",
        "password",
        "secret",
        "secret_ref",
        "passphrase_secret_ref",
        "key_file",
        "known_hosts_file",
        "connection_id",
    ]);

    for path in json_files(temp.path()) {
        let value: Value = serde_json::from_slice(&fs::read(&path).expect("read cache metadata"))
            .expect("valid cache metadata json");
        assert_no_forbidden_keys(&value, &forbidden_keys, &path);
        let serialized = serde_json::to_string(&value).expect("serialize metadata");
        assert!(!serialized.contains("LOG_QUERY_MCP_TEST_PASSWORD"));
        assert!(!serialized.contains("do-not-store-me"));
        assert!(!serialized.contains("/srv/logs/orders"));
    }

    for path in all_paths(temp.path()) {
        let relative = path.strip_prefix(temp.path()).expect("cache relative path");
        let rendered = relative.to_string_lossy();
        assert!(!rendered.contains("/srv/logs/orders"));
        assert!(!rendered.contains("application.log"));
    }
}

fn json_files(root: &Path) -> Vec<PathBuf> {
    all_paths(root)
        .into_iter()
        .filter(|path| path.extension().is_some_and(|value| value == "json"))
        .collect()
}

fn all_paths(root: &Path) -> Vec<PathBuf> {
    let mut output = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory).expect("read cache directory") {
            let path = entry.expect("cache entry").path();
            let metadata = fs::symlink_metadata(&path).expect("cache entry metadata");
            output.push(path.clone());
            if metadata.is_dir() {
                stack.push(path);
            }
        }
    }
    output
}

fn assert_no_forbidden_keys(value: &Value, forbidden: &BTreeSet<&str>, path: &Path) {
    match value {
        Value::Object(object) => {
            for (key, nested) in object {
                assert!(
                    !forbidden.contains(key.as_str()),
                    "forbidden metadata key {key} found in {}",
                    path.display()
                );
                assert_no_forbidden_keys(nested, forbidden, path);
            }
        }
        Value::Array(values) => {
            for nested in values {
                assert_no_forbidden_keys(nested, forbidden, path);
            }
        }
        _ => {}
    }
}
