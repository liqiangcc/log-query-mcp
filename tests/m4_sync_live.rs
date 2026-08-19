use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
};

use log_query_mcp::{
    AppConfigV2, CacheCoverage, CacheStore, RemoteSyncTarget, SyncAction, SyncEngine,
    SyncGenerationReason,
};
use serde_json::json;
use tempfile::TempDir;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

#[tokio::test]
#[ignore = "requires the OpenSSH/SFTP fixture from ssh-research.yml"]
async fn real_sftp_bootstrap_then_incremental_append_preserves_generation() {
    let port: u16 = required("M2_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let remote_dir = required("M2_REMOTE_DIR");
    let local_file = required("M4_LOCAL_SYNC_FILE");

    fs::write(&local_file, b"first\n").expect("reset live remote fixture");

    let cache_dir = TempDir::new().expect("cache tempdir");
    let config_json = json!({
        "version": 2,
        "connections": [{
            "connection_id": "m4-live",
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
        "sources": [{
            "source_id": "m4-live-source",
            "name": "M4 live source",
            "service": "m4-live",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": "m4-live"
            },
            "root": remote_dir,
            "files": ["sync-live.log"],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": cache_dir.path(),
            "max_bytes": 10485760,
            "max_bytes_per_source": 5242880,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": 1048576,
            "max_remote_files_per_source": 10
        }
    });
    let config =
        AppConfigV2::from_json_str(&config_json.to_string()).expect("valid M4 live config");
    let cache =
        CacheStore::from_config(config.cache.as_ref().expect("cache config")).expect("cache");
    let engine = SyncEngine::from_config(&config, cache.clone()).expect("sync engine");
    let target = RemoteSyncTarget::from_source(&config.sources[0], "sync-live.log")
        .expect("remote sync target");

    let first = engine.sync(&target).await.expect("initial bootstrap");
    assert_eq!(
        first.action,
        SyncAction::NewGeneration(SyncGenerationReason::InitialBootstrap)
    );
    assert_eq!(first.coverage, CacheCoverage::Full);
    assert_eq!(first.cached_bytes_written, 6);

    let mut fixture = OpenOptions::new()
        .append(true)
        .open(&local_file)
        .expect("open live remote fixture for append");
    fixture.write_all(b"second\n").expect("append remote data");
    fixture.sync_all().expect("sync remote fixture");
    drop(fixture);

    let second = engine.sync(&target).await.expect("incremental append");
    assert_eq!(second.action, SyncAction::Appended);
    assert_eq!(second.generation, first.generation);
    assert_eq!(second.cached_bytes_written, 7);

    let third = engine.sync(&target).await.expect("unchanged refresh");
    assert_eq!(third.action, SyncAction::Unchanged);
    assert_eq!(third.generation, first.generation);
    assert_eq!(third.cached_bytes_written, 0);

    let mut pinned = cache
        .pin_current_generation("m4-live-source", "sync-live.log")
        .expect("pin current generation");
    let mut text = String::new();
    pinned.read_to_string(&mut text).expect("read cached log");
    assert_eq!(text, "first\nsecond\n");
}
