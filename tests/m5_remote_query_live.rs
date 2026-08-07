use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Arc,
};

use log_query_mcp::{
    AppConfigV2, SourceRegistry, StatefulContextRequest, StatefulContextService, StatefulQueryError,
    StatefulQueryRequest, StatefulQueryService, ToolError, ToolErrorCode,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn connection(port: u16, known_hosts: &str) -> Value {
    json!({
        "connection_id": "m5-live",
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
    })
}

fn cache(root: &Path) -> Value {
    json!({
        "root": root,
        "max_bytes": 10485760,
        "max_bytes_per_source": 5242880,
        "retention_hours": 24,
        "max_generations_per_file": 4
    })
}

fn limits() -> Value {
    json!({
        "max_concurrent_ssh_connections": 2,
        "max_sync_bytes_per_query": 1048576,
        "max_remote_files_per_source": 20
    })
}

fn remote_file_source(source_id: &str, root: &str, file: &str, bootstrap: Value) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": "m5-live"
        },
        "root": root,
        "files": [file],
        "sync": {
            "freshness": "on_query",
            "bootstrap": bootstrap,
            "allow_stale_on_error": false
        }
    })
}

fn directory_source(source_id: &str, root: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {
            "type": "ssh",
            "connection_id": "m5-live"
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

fn local_source(source_id: &str, root: &Path, file: &str) -> Value {
    json!({
        "source_id": source_id,
        "name": source_id,
        "service": source_id,
        "environment": "test",
        "backend": {"type": "local"},
        "root": root,
        "files": [file]
    })
}

fn service(config: Value) -> StatefulQueryService {
    let config = AppConfigV2::from_json_str(&config.to_string()).expect("valid M5 live config");
    let registry = SourceRegistry::from_config_v2(config).expect("M5 registry");
    StatefulQueryService::new(Arc::new(registry)).expect("M5 query service")
}

fn remote_config(
    port: u16,
    known_hosts: &str,
    cache_root: &Path,
    sources: Vec<Value>,
) -> Value {
    json!({
        "version": 2,
        "connections": [connection(port, known_hosts)],
        "sources": sources,
        "cache": cache(cache_root),
        "limits": limits()
    })
}

fn contents(page: &log_query_mcp::StatefulQueryPage) -> Vec<&str> {
    page.results
        .iter()
        .map(|result| result.content.as_str())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the OpenSSH/SFTP fixture from ssh-research.yml"]
async fn remote_query_end_to_end_preserves_snapshots_and_context() {
    let port: u16 = required("M2_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let remote_dir = required("M2_REMOTE_DIR");
    let query_file = required("M5_LOCAL_QUERY_FILE");
    let partial_file = required("M5_LOCAL_PARTIAL_FILE");
    let directory_root = required("M5_REMOTE_DIRECTORY_ROOT");

    fs::write(
        &query_file,
        concat!(
            "2026-08-07T13:00:01Z MATCH one\n",
            "2026-08-07T13:00:02Z MATCH two\n",
            "2026-08-07T13:00:03Z KEEP old-generation\n"
        ),
    )
    .expect("reset query fixture");
    fs::write(
        &partial_file,
        b"old-history-that-must-not-be-silently-ignored\nRECENT partial\n",
    )
    .expect("reset partial fixture");

    // Full bootstrap: first page freezes a cache snapshot. Appending the remote file must not
    // become visible through an existing cursor, but a new query must refresh incrementally.
    let cache_dir = TempDir::new().expect("query cache");
    let query_service = service(remote_config(
        port,
        &known_hosts,
        cache_dir.path(),
        vec![remote_file_source(
            "remote-query",
            &remote_dir,
            "query-live.log",
            json!({"type": "full"}),
        )],
    ));

    let first = query_service
        .search(
            StatefulQueryRequest::new(vec!["remote-query".to_owned()], "MATCH")
                .with_max_results(1),
        )
        .await
        .expect("remote first page");
    assert_eq!(contents(&first), vec!["2026-08-07T13:00:01Z MATCH one"]);
    let cursor = first.next_cursor.clone().expect("remote cursor");

    let mut remote = OpenOptions::new()
        .append(true)
        .open(&query_file)
        .expect("open query fixture for append");
    remote
        .write_all(b"2026-08-07T13:00:04Z MATCH appended-after-cursor\n")
        .expect("append remote query fixture");
    remote.sync_all().expect("sync query fixture");
    drop(remote);

    let second = query_service
        .search(
            StatefulQueryRequest::new(vec!["remote-query".to_owned()], "MATCH")
                .with_max_results(10)
                .with_cursor(cursor),
        )
        .await
        .expect("cursor should reuse first-page snapshot");
    assert_eq!(contents(&second), vec!["2026-08-07T13:00:02Z MATCH two"]);
    assert!(second.next_cursor.is_none());

    let refreshed = query_service
        .search(
            StatefulQueryRequest::new(vec!["remote-query".to_owned()], "MATCH")
                .with_max_results(10),
        )
        .await
        .expect("new query should refresh remote cache");
    assert_eq!(
        contents(&refreshed),
        vec![
            "2026-08-07T13:00:01Z MATCH one",
            "2026-08-07T13:00:02Z MATCH two",
            "2026-08-07T13:00:04Z MATCH appended-after-cursor",
        ]
    );

    // Directory discovery is admin-configured, non-recursive, suffix-filtered, and stable.
    let directory_cache = TempDir::new().expect("directory cache");
    let directory_service = service(remote_config(
        port,
        &known_hosts,
        directory_cache.path(),
        vec![directory_source("remote-directory", &directory_root)],
    ));
    let directory_page = directory_service
        .search(StatefulQueryRequest::new(
            vec!["remote-directory".to_owned()],
            "DIRMATCH",
        ))
        .await
        .expect("directory query");
    assert_eq!(directory_page.results.len(), 1);
    assert!(directory_page.results[0].content.contains("DIRMATCH included"));
    assert!(directory_page.results[0].file_name.ends_with("included.log"));

    // A single query can combine Local and Remote sources without changing query semantics.
    let local_root = TempDir::new().expect("local source");
    fs::write(local_root.path().join("local.log"), "MIXED local\n").expect("local fixture");
    let mixed_cache = TempDir::new().expect("mixed cache");
    let mixed_service = service(remote_config(
        port,
        &known_hosts,
        mixed_cache.path(),
        vec![
            local_source("local-mixed", local_root.path(), "local.log"),
            directory_source("remote-mixed", &directory_root),
        ],
    ));
    let mixed = mixed_service
        .search(
            StatefulQueryRequest::new(
                vec!["local-mixed".to_owned(), "remote-mixed".to_owned()],
                "MIXED",
            )
            .with_max_results(10),
        )
        .await
        .expect("mixed Local+Remote query");
    assert_eq!(mixed.results.len(), 2);
    assert!(mixed.results.iter().any(|result| result.content == "MIXED local"));
    assert!(
        mixed
            .results
            .iter()
            .any(|result| result.content.contains("MIXED remote"))
    );

    // Tail and from_now caches that start after remote offset zero are incomplete. They must
    // return CACHE_SCOPE_EXCEEDED instead of a potentially false empty result.
    for (name, bootstrap) in [
        ("tail", json!({"type": "tail", "bytes": 8})),
        ("from-now", json!({"type": "from_now"})),
    ] {
        let partial_cache = TempDir::new().expect("partial cache");
        let partial_service = service(remote_config(
            port,
            &known_hosts,
            partial_cache.path(),
            vec![remote_file_source(
                &format!("partial-{name}"),
                &remote_dir,
                "partial-live.log",
                bootstrap,
            )],
        ));
        let error = partial_service
            .search(StatefulQueryRequest::new(
                vec![format!("partial-{name}")],
                "definitely-not-present",
            ))
            .await
            .expect_err("partial cache must not produce a false empty result");
        assert!(matches!(error, StatefulQueryError::CacheScopeExceeded));
        assert_eq!(ToolError::from(error).code, ToolErrorCode::CacheScopeExceeded);
    }

    // Keep a match_ref on the old generation, force a replacement generation, then make future
    // SSH authentication impossible by temporarily removing known_hosts. Context must still be
    // served entirely from the pinned local cache generation.
    let keep = query_service
        .search(StatefulQueryRequest::new(
            vec!["remote-query".to_owned()],
            "KEEP",
        ))
        .await
        .expect("old-generation match");
    let match_ref = keep.results[0].match_ref.clone();

    fs::write(
        &query_file,
        b"2026-08-07T13:10:00Z NEW replacement-generation\n",
    )
    .expect("replace remote file");
    let replacement = query_service
        .search(StatefulQueryRequest::new(
            vec!["remote-query".to_owned()],
            "NEW",
        ))
        .await
        .expect("replacement query");
    assert_eq!(replacement.results.len(), 1);

    let disabled_known_hosts = format!("{known_hosts}.m5-disabled");
    fs::rename(&known_hosts, &disabled_known_hosts).expect("disable future SSH host verification");
    let context_service = StatefulContextService::from_query_service(&query_service)
        .expect("context service from query service");
    let context = context_service
        .get_context(StatefulContextRequest::new(match_ref).with_lines(0, 0))
        .await
        .expect("old match_ref context must be cache-only");
    fs::rename(&disabled_known_hosts, &known_hosts).expect("restore known_hosts fixture");

    assert_eq!(context.lines.len(), 1);
    assert!(context.lines[0].content.contains("KEEP old-generation"));
}
