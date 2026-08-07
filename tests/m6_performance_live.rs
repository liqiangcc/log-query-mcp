use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use log_query_mcp::{
    AppConfigV2, CONTINUITY_FINGERPRINT_WINDOW_BYTES, CacheStore, RemoteSyncTarget, ScanLimits,
    ScanRequest, SyncAction, SyncEngine, scan_reader,
};
use serde_json::{Value, json};

const SOURCE_ID: &str = "m6-perf";
const REMOTE_FILE: &str = "performance.log";
const PERF_MARKER: &[u8] = b"M6_PERF_MARKER\n";
const APPEND_MARKER: &[u8] = b"M6_PERF_APPEND_MARKER\n";
const MIB: u64 = 1024 * 1024;
const FILL_BUFFER_BYTES: usize = 1024 * 1024;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required benchmark environment variable {name}"))
}

fn required_u64(name: &str) -> u64 {
    required(name)
        .parse()
        .unwrap_or_else(|_| panic!("invalid integer benchmark environment variable {name}"))
}

fn config(
    cache_root: &Path,
    remote_dir: &str,
    logical_size: u64,
    cached_bytes: u64,
    append_bytes: u64,
    bootstrap: Value,
) -> AppConfigV2 {
    let port: u16 = required("M2_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let capacity = cached_bytes
        .checked_add(append_bytes)
        .and_then(|value| value.checked_add(128 * MIB))
        .expect("benchmark cache capacity");
    let max_sync_payload = cached_bytes.max(append_bytes).max(MIB);
    let max_sync_bytes = max_sync_payload
        .checked_add(2 * CONTINUITY_FINGERPRINT_WINDOW_BYTES)
        .and_then(|value| value.checked_add(MIB))
        .expect("benchmark sync budget");

    let value = json!({
        "version": 2,
        "connections": [{
            "connection_id": "m6-perf-server",
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
            "operation_timeout_millis": 600000,
            "keepalive_seconds": 30
        }],
        "sources": [{
            "source_id": SOURCE_ID,
            "name": "M6 performance source",
            "service": "m6-perf",
            "environment": "benchmark",
            "backend": {
                "type": "ssh",
                "connection_id": "m6-perf-server"
            },
            "root": remote_dir,
            "files": [REMOTE_FILE],
            "sync": {
                "freshness": "on_query",
                "bootstrap": bootstrap,
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": cache_root,
            "max_bytes": capacity,
            "max_bytes_per_source": capacity,
            "retention_hours": 24,
            "max_generations_per_file": 4
        },
        "limits": {
            "max_concurrent_ssh_connections": 2,
            "max_sync_bytes_per_query": max_sync_bytes,
            "max_remote_files_per_source": 10,
            "max_scan_bytes_per_page": logical_size.max(MIB),
            "query_timeout_millis": 600000
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid M6 performance config")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the OpenSSH performance fixture from m6-performance.yml"]
async fn sync_and_scan_profile() {
    let profile = required("M6_PERF_PROFILE");
    let mode = required("M6_PERF_BOOTSTRAP");
    let logical_size = required_u64("M6_PERF_SIZE_BYTES");
    let tail_bytes = required_u64("M6_PERF_TAIL_BYTES");
    let append_bytes = required_u64("M6_PERF_APPEND_BYTES");
    let local_file = PathBuf::from(required("M6_PERF_LOCAL_FILE"));
    let remote_dir = required("M6_PERF_REMOTE_DIR");
    let cache_root = PathBuf::from(required("M6_PERF_CACHE_ROOT"));

    assert!(logical_size > PERF_MARKER.len() as u64);
    if cache_root.exists() {
        fs::remove_dir_all(&cache_root).expect("clear benchmark cache");
    }

    let (bootstrap, dense_start, expected_cached_bytes) = match mode.as_str() {
        "full" => (json!({"type": "full"}), 0, logical_size),
        "tail" => {
            assert!(tail_bytes > PERF_MARKER.len() as u64);
            let cached = tail_bytes.min(logical_size);
            (
                json!({"type": "tail", "bytes": tail_bytes}),
                logical_size - cached,
                cached,
            )
        }
        other => panic!("unsupported M6_PERF_BOOTSTRAP {other}"),
    };

    prepare_fixture(&local_file, logical_size, dense_start, PERF_MARKER);
    let app_config = config(
        &cache_root,
        &remote_dir,
        logical_size,
        expected_cached_bytes,
        append_bytes,
        bootstrap,
    );
    let cache =
        CacheStore::from_config(app_config.cache.as_ref().expect("cache config")).expect("cache");
    let engine = SyncEngine::from_config(&app_config, cache.clone()).expect("sync engine");
    let target = RemoteSyncTarget::from_source(&app_config.sources[0], REMOTE_FILE)
        .expect("performance sync target");

    let started = Instant::now();
    let bootstrap_outcome = engine.sync(&target).await.expect("cold bootstrap");
    let bootstrap_elapsed = started.elapsed();
    assert!(matches!(
        bootstrap_outcome.action,
        SyncAction::NewGeneration(_)
    ));
    assert_eq!(bootstrap_outcome.remote_size, logical_size);
    assert_eq!(
        bootstrap_outcome.cached_bytes_written,
        expected_cached_bytes
    );
    emit_metric(
        &profile,
        "cold_bootstrap",
        logical_size,
        bootstrap_elapsed.as_millis(),
        bootstrap_outcome.remote_bytes_read,
        bootstrap_outcome.cached_bytes_written,
        bootstrap_outcome.cached_range.start,
        bootstrap_outcome.cached_range.end_exclusive,
        cache_disk_bytes(&cache_root),
        None,
    );

    let started = Instant::now();
    let unchanged = engine.sync(&target).await.expect("unchanged refresh");
    let unchanged_elapsed = started.elapsed();
    assert_eq!(unchanged.action, SyncAction::Unchanged);
    assert_eq!(unchanged.cached_bytes_written, 0);
    assert!(unchanged.remote_bytes_read <= CONTINUITY_FINGERPRINT_WINDOW_BYTES);
    emit_metric(
        &profile,
        "unchanged_probe",
        logical_size,
        unchanged_elapsed.as_millis(),
        unchanged.remote_bytes_read,
        unchanged.cached_bytes_written,
        unchanged.cached_range.start,
        unchanged.cached_range.end_exclusive,
        cache_disk_bytes(&cache_root),
        None,
    );

    let mut generation = cache
        .pin_current_generation(SOURCE_ID, REMOTE_FILE)
        .expect("pin current generation");
    let scan_limits = ScanLimits {
        max_scan_bytes: expected_cached_bytes.max(1),
        max_results: 10,
        max_line_bytes: 4096,
        max_returned_content_bytes: 64 * 1024,
        read_buffer_bytes: 1024 * 1024,
    };
    let scan_request = ScanRequest::new("M6_PERF_MARKER").with_limits(scan_limits);
    let started = Instant::now();
    let scan = scan_reader(&mut generation, &scan_request).expect("local cache scan");
    let scan_elapsed = started.elapsed();
    assert_eq!(scan.results.len(), 1);
    emit_metric(
        &profile,
        "cache_local_scan",
        logical_size,
        scan_elapsed.as_millis(),
        0,
        0,
        bootstrap_outcome.cached_range.start,
        bootstrap_outcome.cached_range.end_exclusive,
        cache_disk_bytes(&cache_root),
        Some(scan.bytes_scanned),
    );

    if append_bytes > 0 {
        append_fixture(&local_file, append_bytes, APPEND_MARKER);
        let started = Instant::now();
        let appended = engine.sync(&target).await.expect("incremental append");
        let append_elapsed = started.elapsed();
        assert_eq!(appended.action, SyncAction::Appended);
        assert_eq!(appended.cached_bytes_written, append_bytes);
        assert!(
            appended.remote_bytes_read
                <= append_bytes + 2 * CONTINUITY_FINGERPRINT_WINDOW_BYTES
        );
        emit_metric(
            &profile,
            "incremental_append",
            logical_size + append_bytes,
            append_elapsed.as_millis(),
            appended.remote_bytes_read,
            appended.cached_bytes_written,
            appended.cached_range.start,
            appended.cached_range.end_exclusive,
            cache_disk_bytes(&cache_root),
            None,
        );
    }
}

fn prepare_fixture(path: &Path, logical_size: u64, dense_start: u64, marker: &[u8]) {
    assert!(dense_start <= logical_size);
    let dense_bytes = logical_size - dense_start;
    assert!(dense_bytes >= marker.len() as u64);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create performance fixture directory");
    }
    let file = File::create(path).expect("create performance fixture");
    file.set_len(dense_start)
        .expect("create sparse benchmark prefix");
    let mut writer = BufWriter::with_capacity(FILL_BUFFER_BYTES, file);
    writer
        .seek(SeekFrom::Start(dense_start))
        .expect("seek benchmark dense range");
    write_dense_region(&mut writer, dense_bytes, marker);
    writer.flush().expect("flush performance fixture");
}

fn append_fixture(path: &Path, bytes: u64, marker: &[u8]) {
    assert!(bytes >= marker.len() as u64);
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open benchmark fixture for append");
    let mut writer = BufWriter::with_capacity(FILL_BUFFER_BYTES, file);
    write_dense_region(&mut writer, bytes, marker);
    writer.flush().expect("flush benchmark append");
}

fn write_dense_region<W: Write>(writer: &mut W, bytes: u64, marker: &[u8]) {
    let filler_bytes = bytes - marker.len() as u64;
    let line = b"M6_PERF_FILLER 0123456789abcdef0123456789abcdef0123456789abcdef\n";
    let mut block = Vec::with_capacity(FILL_BUFFER_BYTES);
    while block.len() + line.len() <= FILL_BUFFER_BYTES {
        block.extend_from_slice(line);
    }

    let mut remaining = filler_bytes;
    while remaining > 0 {
        let chunk = remaining.min(block.len() as u64) as usize;
        writer
            .write_all(&block[..chunk])
            .expect("write benchmark filler");
        remaining -= chunk as u64;
    }
    writer.write_all(marker).expect("write benchmark marker");
}

#[allow(clippy::too_many_arguments)]
fn emit_metric(
    profile: &str,
    scenario: &str,
    remote_size: u64,
    elapsed_ms: u128,
    remote_bytes_read: u64,
    cached_bytes_written: u64,
    cached_start: u64,
    cached_end: u64,
    cache_disk_bytes: u64,
    bytes_scanned: Option<u64>,
) {
    let value = json!({
        "profile": profile,
        "scenario": scenario,
        "remote_size_bytes": remote_size,
        "elapsed_ms": elapsed_ms,
        "remote_bytes_read": remote_bytes_read,
        "cached_bytes_written": cached_bytes_written,
        "cached_range_start": cached_start,
        "cached_range_end": cached_end,
        "cache_disk_bytes": cache_disk_bytes,
        "bytes_scanned": bytes_scanned,
    });
    println!("M6_PERF_METRIC {}", value);
}

fn cache_disk_bytes(root: &Path) -> u64 {
    if !root.exists() {
        return 0;
    }
    let mut total = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory).expect("read benchmark cache directory") {
            let path = entry.expect("benchmark cache entry").path();
            let metadata = fs::symlink_metadata(&path).expect("benchmark cache metadata");
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}
