#![cfg(target_os = "linux")]

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use log_query_mcp::{
    AppConfigV2, CacheCoverage, CacheStore, RemoteSyncTarget, SyncAction, SyncEngine,
    SyncGenerationReason,
};
use serde_json::{Value, json};
use tempfile::TempDir;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn port() -> u16 {
    required("M7_SYNC_SSH_PORT")
        .parse()
        .expect("valid M7 sync SSH port")
}

fn connection() -> Value {
    json!({
        "connection_id": "m7-proxy-sync",
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port(),
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M7_SYNC_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": required("M7_SYNC_KNOWN_HOSTS")
        },
        "proxy": {
            "type": "command",
            "program": required("M7_SYNC_PROXY_PROGRAM"),
            "args": ["{host}", "{port}"]
        },
        "connect_timeout_millis": 5000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 5
    })
}

fn config(
    source_id: &str,
    remote_identifier: &str,
    bootstrap: Value,
    cache_root: &Path,
) -> AppConfigV2 {
    let document = json!({
        "version": 2,
        "connections": [connection()],
        "sources": [{
            "source_id": source_id,
            "name": source_id,
            "service": "m7-proxy-sync",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": "m7-proxy-sync"
            },
            "root": required("M7_SYNC_REMOTE_DIR"),
            "files": [remote_identifier],
            "sync": {
                "freshness": "on_query",
                "bootstrap": bootstrap,
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": cache_root,
            "max_bytes": 16 * 1024 * 1024,
            "max_bytes_per_source": 4 * 1024 * 1024,
            "retention_hours": 24,
            "max_generations_per_file": 6
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 2 * 1024 * 1024,
            "max_remote_files_per_source": 20
        }
    });
    AppConfigV2::from_json_str(&document.to_string()).expect("valid M7 Proxy sync config")
}

fn engine_and_target(
    source_id: &str,
    remote_identifier: &str,
    bootstrap: Value,
    cache_root: &Path,
) -> (CacheStore, SyncEngine, RemoteSyncTarget) {
    let config = config(source_id, remote_identifier, bootstrap, cache_root);
    let cache =
        CacheStore::from_config(config.cache.as_ref().expect("cache config")).expect("cache");
    let engine = SyncEngine::from_config(&config, cache.clone()).expect("sync engine");
    let target = RemoteSyncTarget::from_source(&config.sources[0], remote_identifier)
        .expect("remote sync target");
    (cache, engine, target)
}

fn cached_text(cache: &CacheStore, source_id: &str, remote_identifier: &str) -> String {
    let mut pinned = cache
        .pin_current_generation(source_id, remote_identifier)
        .expect("pin current generation");
    let mut text = String::new();
    pinned.read_to_string(&mut text).expect("read cached text");
    text
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Sync OpenSSH fixture"]
async fn full_bootstrap_and_incremental_append_stay_on_one_generation() {
    let local_file = required("M7_SYNC_FULL_FILE");
    fs::write(&local_file, b"first\n").expect("reset full fixture");
    let cache_dir = TempDir::new().expect("full cache");
    let (cache, engine, target) = engine_and_target(
        "m7-sync-full",
        "sync-full.log",
        json!({"type": "full"}),
        cache_dir.path(),
    );

    let first = engine.sync(&target).await.expect("full bootstrap");
    assert_eq!(
        first.action,
        SyncAction::NewGeneration(SyncGenerationReason::InitialBootstrap)
    );
    assert_eq!(first.coverage, CacheCoverage::Full);
    assert_eq!(first.cached_bytes_written, 6);

    let mut fixture = OpenOptions::new()
        .append(true)
        .open(&local_file)
        .expect("open full fixture for append");
    fixture.write_all(b"second\n").expect("append full fixture");
    fixture.sync_all().expect("sync full fixture");
    drop(fixture);

    let second = engine.sync(&target).await.expect("incremental append");
    assert_eq!(second.action, SyncAction::Appended);
    assert_eq!(second.generation, first.generation);
    assert_eq!(second.cached_bytes_written, 7);
    assert_eq!(
        cached_text(&cache, "m7-sync-full", "sync-full.log"),
        "first\nsecond\n"
    );
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Sync OpenSSH fixture"]
async fn tail_bootstrap_keeps_only_configured_tail_then_appends() {
    let local_file = required("M7_SYNC_TAIL_FILE");
    let initial = b"old-history\nTAIL one\n";
    let tail = b"TAIL one\n";
    fs::write(&local_file, initial).expect("reset tail fixture");
    let cache_dir = TempDir::new().expect("tail cache");
    let (cache, engine, target) = engine_and_target(
        "m7-sync-tail",
        "sync-tail.log",
        json!({"type": "tail", "bytes": tail.len()}),
        cache_dir.path(),
    );

    let first = engine.sync(&target).await.expect("tail bootstrap");
    let expected_start = (initial.len() - tail.len()) as u64;
    assert_eq!(
        first.coverage,
        CacheCoverage::Tail {
            start_offset: expected_start
        }
    );
    assert_eq!(
        cached_text(&cache, "m7-sync-tail", "sync-tail.log"),
        "TAIL one\n"
    );

    let mut fixture = OpenOptions::new()
        .append(true)
        .open(&local_file)
        .expect("open tail fixture for append");
    fixture
        .write_all(b"TAIL two\n")
        .expect("append tail fixture");
    fixture.sync_all().expect("sync tail fixture");
    drop(fixture);

    let second = engine.sync(&target).await.expect("tail incremental append");
    assert_eq!(second.action, SyncAction::Appended);
    assert_eq!(second.generation, first.generation);
    assert_eq!(
        cached_text(&cache, "m7-sync-tail", "sync-tail.log"),
        "TAIL one\nTAIL two\n"
    );
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Sync OpenSSH fixture"]
async fn from_now_bootstrap_excludes_history_and_captures_future_append() {
    let local_file = required("M7_SYNC_FROM_NOW_FILE");
    let history = b"history-before-from-now\n";
    fs::write(&local_file, history).expect("reset from-now fixture");
    let cache_dir = TempDir::new().expect("from-now cache");
    let (cache, engine, target) = engine_and_target(
        "m7-sync-from-now",
        "sync-from-now.log",
        json!({"type": "from_now"}),
        cache_dir.path(),
    );

    let first = engine.sync(&target).await.expect("from-now bootstrap");
    assert_eq!(
        first.coverage,
        CacheCoverage::FromNow {
            start_offset: history.len() as u64
        }
    );
    assert_eq!(first.cached_bytes_written, 0);
    assert_eq!(
        cached_text(&cache, "m7-sync-from-now", "sync-from-now.log"),
        ""
    );

    let mut fixture = OpenOptions::new()
        .append(true)
        .open(&local_file)
        .expect("open from-now fixture for append");
    fixture
        .write_all(b"FROMNOW new\n")
        .expect("append from-now fixture");
    fixture.sync_all().expect("sync from-now fixture");
    drop(fixture);

    let second = engine.sync(&target).await.expect("from-now append");
    assert_eq!(second.action, SyncAction::Appended);
    assert_eq!(second.generation, first.generation);
    assert_eq!(
        cached_text(&cache, "m7-sync-from-now", "sync-from-now.log"),
        "FROMNOW new\n"
    );
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Sync OpenSSH fixture"]
async fn truncate_creates_a_new_generation() {
    let local_file = required("M7_SYNC_TRUNCATE_FILE");
    fs::write(&local_file, b"TRUNCATE before before before\n").expect("reset truncate fixture");
    let cache_dir = TempDir::new().expect("truncate cache");
    let (cache, engine, target) = engine_and_target(
        "m7-sync-truncate",
        "sync-truncate.log",
        json!({"type": "full"}),
        cache_dir.path(),
    );

    let first = engine
        .sync(&target)
        .await
        .expect("truncate initial bootstrap");
    fs::write(&local_file, b"short\n").expect("truncate remote fixture");
    let second = engine.sync(&target).await.expect("sync after truncate");

    assert_eq!(
        second.action,
        SyncAction::NewGeneration(SyncGenerationReason::RemoteTruncated)
    );
    assert_ne!(second.generation, first.generation);
    assert_eq!(
        cached_text(&cache, "m7-sync-truncate", "sync-truncate.log"),
        "short\n"
    );
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Sync OpenSSH fixture"]
async fn same_path_rotation_with_same_size_content_uses_continuity_mismatch_generation() {
    let local_file = required("M7_SYNC_ROTATE_FILE");
    let rotated_file = format!("{local_file}.1");
    let _ = fs::remove_file(&rotated_file);
    let old = b"ROTATE old-aaaaaaaaaaaaaaaa\n";
    let new = b"ROTATE new-bbbbbbbbbbbbbbbb\n";
    assert_eq!(old.len(), new.len());
    fs::write(&local_file, old).expect("reset rotation fixture");
    let cache_dir = TempDir::new().expect("rotation cache");
    let (cache, engine, target) = engine_and_target(
        "m7-sync-rotate",
        "sync-rotate.log",
        json!({"type": "full"}),
        cache_dir.path(),
    );

    let first = engine
        .sync(&target)
        .await
        .expect("rotation initial bootstrap");
    fs::rename(&local_file, &rotated_file).expect("rotate old remote file");
    fs::write(&local_file, new).expect("create replacement remote file");
    let second = engine
        .sync(&target)
        .await
        .expect("sync replacement generation");

    assert_eq!(
        second.action,
        SyncAction::NewGeneration(SyncGenerationReason::ContinuityMismatch)
    );
    assert_ne!(second.generation, first.generation);
    assert_eq!(
        cached_text(&cache, "m7-sync-rotate", "sync-rotate.log"),
        "ROTATE new-bbbbbbbbbbbbbbbb\n"
    );
}
