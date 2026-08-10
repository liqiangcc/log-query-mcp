#![cfg(target_os = "linux")]

use std::{
    fs::{self, File, OpenOptions},
    io::{BufWriter, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::Instant,
};

use log_query_mcp::{
    AppConfigV2, CONTINUITY_FINGERPRINT_WINDOW_BYTES, CacheStore, RemoteSyncTarget, ScanLimits,
    ScanRequest, SshConnectionManager, SyncAction, SyncEngine, scan_reader,
};
use serde_json::{Value, json};

const REMOTE_FILE: &str = "performance.log";
const SMALL_FILE: &str = "transport.log";
const PERF_MARKER: &[u8] = b"M7_PERF_MARKER\n";
const APPEND_MARKER: &[u8] = b"M7_PERF_APPEND_MARKER\n";
const MIB: u64 = 1024 * 1024;
const FILL_BUFFER_BYTES: usize = 1024 * 1024;
const SETUP_SAMPLES: usize = 5;
const RANGE_READ_SAMPLES: usize = 300;

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required M7 performance environment variable {name}"))
}

fn required_u64(name: &str) -> u64 {
    required(name)
        .parse()
        .unwrap_or_else(|_| panic!("invalid integer M7 performance environment variable {name}"))
}

fn ssh_connection(connection_id: &str, transport: &str) -> Value {
    let port: u16 = required("M7_PERF_SSH_PORT").parse().expect("SSH port");
    let known_hosts = required("M7_PERF_KNOWN_HOSTS");
    let mut connection = json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port,
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M7_PERF_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "connect_timeout_millis": 5000,
        "operation_timeout_millis": 600000,
        "keepalive_seconds": 30
    });
    match transport {
        "direct" => {}
        "proxy" => {
            connection["proxy"] = json!({
                "type": "command",
                "program": required("M7_PERF_PROXY_PROGRAM"),
                "args": ["{host}", "{port}"]
            });
        }
        other => panic!("unsupported transport {other}"),
    }
    connection
}

fn sync_config(
    cache_root: &Path,
    remote_dir: &str,
    transport: &str,
    logical_size: u64,
    cached_bytes: u64,
    append_bytes: u64,
    bootstrap: Value,
) -> AppConfigV2 {
    let capacity = cached_bytes
        .checked_add(append_bytes)
        .and_then(|value| value.checked_add(128 * MIB))
        .expect("benchmark cache capacity");
    let max_sync_payload = cached_bytes.max(append_bytes).max(MIB);
    let max_sync_bytes = max_sync_payload
        .checked_add(2 * CONTINUITY_FINGERPRINT_WINDOW_BYTES)
        .and_then(|value| value.checked_add(MIB))
        .expect("benchmark sync budget");
    let source_id = format!("m7-perf-{transport}");
    let connection_id = format!("m7-perf-{transport}-server");

    let value = json!({
        "version": 2,
        "connections": [ssh_connection(&connection_id, transport)],
        "sources": [{
            "source_id": source_id,
            "name": format!("M7 {transport} performance source"),
            "service": "m7-perf",
            "environment": "benchmark",
            "backend": {
                "type": "ssh",
                "connection_id": connection_id
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
            "max_concurrent_ssh_connections": 4,
            "max_sync_bytes_per_query": max_sync_bytes,
            "max_remote_files_per_source": 10,
            "max_scan_bytes_per_page": logical_size.max(MIB),
            "query_timeout_millis": 600000
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid M7 performance config")
}

fn transport_config(remote_dir: &str, cache_root: &Path) -> AppConfigV2 {
    let value = json!({
        "version": 2,
        "connections": [
            ssh_connection("m7-perf-direct", "direct"),
            ssh_connection("m7-perf-proxy", "proxy")
        ],
        "sources": [
            {
                "source_id": "m7-perf-direct-source",
                "name": "M7 direct transport source",
                "service": "m7-perf",
                "environment": "benchmark",
                "backend": {"type": "ssh", "connection_id": "m7-perf-direct"},
                "root": remote_dir,
                "files": [SMALL_FILE],
                "sync": {
                    "freshness": "on_query",
                    "bootstrap": {"type": "full"},
                    "allow_stale_on_error": false
                }
            },
            {
                "source_id": "m7-perf-proxy-source",
                "name": "M7 proxy transport source",
                "service": "m7-perf",
                "environment": "benchmark",
                "backend": {"type": "ssh", "connection_id": "m7-perf-proxy"},
                "root": remote_dir,
                "files": [SMALL_FILE],
                "sync": {
                    "freshness": "on_query",
                    "bootstrap": {"type": "full"},
                    "allow_stale_on_error": false
                }
            }
        ],
        "cache": {
            "root": cache_root,
            "max_bytes": 8 * MIB,
            "max_bytes_per_source": 4 * MIB,
            "retention_hours": 1,
            "max_generations_per_file": 2
        },
        "limits": {
            "max_concurrent_ssh_connections": 4,
            "max_sync_bytes_per_query": MIB,
            "max_remote_files_per_source": 10
        }
    });
    AppConfigV2::from_json_str(&value.to_string()).expect("valid M7 transport performance config")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the OpenSSH fixture from m7-proxy-performance.yml"]
async fn sync_and_scan_profile() {
    let profile = required("M7_PERF_PROFILE");
    let transport = required("M7_PERF_TRANSPORT");
    let mode = required("M7_PERF_BOOTSTRAP");
    let logical_size = required_u64("M7_PERF_SIZE_BYTES");
    let tail_bytes = required_u64("M7_PERF_TAIL_BYTES");
    let append_bytes = required_u64("M7_PERF_APPEND_BYTES");
    let local_file = PathBuf::from(required("M7_PERF_LOCAL_FILE"));
    let remote_dir = required("M7_PERF_REMOTE_DIR");
    let cache_root = PathBuf::from(required("M7_PERF_CACHE_ROOT"));

    assert!(logical_size > PERF_MARKER.len() as u64);
    if cache_root.exists() {
        fs::remove_dir_all(&cache_root).expect("clear M7 benchmark cache");
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
        other => panic!("unsupported M7_PERF_BOOTSTRAP {other}"),
    };

    prepare_fixture(&local_file, logical_size, dense_start, PERF_MARKER);
    let app_config = sync_config(
        &cache_root,
        &remote_dir,
        &transport,
        logical_size,
        expected_cached_bytes,
        append_bytes,
        bootstrap,
    );
    let cache =
        CacheStore::from_config(app_config.cache.as_ref().expect("cache config")).expect("cache");
    let engine = SyncEngine::from_config(&app_config, cache.clone()).expect("sync engine");
    let source_id = format!("m7-perf-{transport}");
    let target = RemoteSyncTarget::from_source(&app_config.sources[0], REMOTE_FILE)
        .expect("M7 performance sync target");

    let started = Instant::now();
    let bootstrap_outcome = engine.sync(&target).await.expect("cold bootstrap");
    let bootstrap_elapsed = started.elapsed();
    assert!(matches!(
        bootstrap_outcome.action,
        SyncAction::NewGeneration(_)
    ));
    assert_eq!(bootstrap_outcome.remote_size, logical_size);
    assert_eq!(bootstrap_outcome.cached_bytes_written, expected_cached_bytes);
    assert_eq!(
        bootstrap_outcome.remote_bytes_read,
        expected_cached_bytes + CONTINUITY_FINGERPRINT_WINDOW_BYTES.min(logical_size)
    );
    emit_sync_metric(
        &transport,
        &profile,
        "cold_bootstrap",
        logical_size,
        bootstrap_elapsed.as_millis(),
        bootstrap_outcome.remote_bytes_read,
        bootstrap_outcome.cached_bytes_written,
        cache_disk_bytes(&cache_root),
        None,
    );

    let started = Instant::now();
    let unchanged = engine.sync(&target).await.expect("unchanged refresh");
    let unchanged_elapsed = started.elapsed();
    assert_eq!(unchanged.action, SyncAction::Unchanged);
    assert_eq!(unchanged.cached_bytes_written, 0);
    assert!(unchanged.remote_bytes_read <= CONTINUITY_FINGERPRINT_WINDOW_BYTES);
    emit_sync_metric(
        &transport,
        &profile,
        "unchanged_probe",
        logical_size,
        unchanged_elapsed.as_millis(),
        unchanged.remote_bytes_read,
        0,
        cache_disk_bytes(&cache_root),
        None,
    );

    let mut generation = cache
        .pin_current_generation(&source_id, REMOTE_FILE)
        .expect("pin M7 current generation");
    let scan_limits = ScanLimits {
        max_scan_bytes: expected_cached_bytes.max(1),
        max_results: 10,
        max_line_bytes: 4096,
        max_returned_content_bytes: 64 * 1024,
        read_buffer_bytes: 1024 * 1024,
    };
    let scan_request = ScanRequest::new("M7_PERF_MARKER").with_limits(scan_limits);
    let started = Instant::now();
    let scan = scan_reader(&mut generation, &scan_request).expect("local cache scan");
    let scan_elapsed = started.elapsed();
    assert_eq!(scan.results.len(), 1);
    emit_sync_metric(
        &transport,
        &profile,
        "cache_local_scan",
        logical_size,
        scan_elapsed.as_millis(),
        0,
        0,
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
            appended.remote_bytes_read <= append_bytes + 2 * CONTINUITY_FINGERPRINT_WINDOW_BYTES
        );
        emit_sync_metric(
            &transport,
            &profile,
            "incremental_append",
            logical_size + append_bytes,
            append_elapsed.as_millis(),
            appended.remote_bytes_read,
            appended.cached_bytes_written,
            cache_disk_bytes(&cache_root),
            None,
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires the OpenSSH fixture from m7-proxy-performance.yml"]
async fn transport_setup_range_reads_and_mixed_concurrency() {
    let remote_dir = required("M7_PERF_REMOTE_DIR");
    let local_small_file = PathBuf::from(required("M7_PERF_LOCAL_SMALL_FILE"));
    let remote_small_file = format!("{remote_dir}/{SMALL_FILE}");
    fs::write(&local_small_file, b"hello world from M7 performance\n")
        .expect("write small M7 performance fixture");

    let cache_root = PathBuf::from(required("M7_PERF_TRANSPORT_CACHE_ROOT"));
    if cache_root.exists() {
        fs::remove_dir_all(&cache_root).expect("clear M7 transport cache");
    }
    let config = transport_config(&remote_dir, &cache_root);
    let manager = SshConnectionManager::from_config(&config).expect("M7 performance manager");

    let direct_setup = measure_setup(&manager, "m7-perf-direct", &remote_small_file).await;
    let proxy_setup = measure_setup(&manager, "m7-perf-proxy", &remote_small_file).await;
    emit_transport_metric("direct_setup", SETUP_SAMPLES, direct_setup);
    emit_transport_metric("proxy_setup", SETUP_SAMPLES, proxy_setup);

    let proxy_reader = manager
        .open_reader("m7-perf-proxy")
        .await
        .expect("open ProxyCommand reader for range-read regression");
    let range_started = Instant::now();
    for _ in 0..RANGE_READ_SAMPLES {
        let bytes = proxy_reader
            .read_range(&remote_small_file, 6, 5)
            .await
            .expect("ProxyCommand bounded range read");
        assert_eq!(bytes, b"world");
    }
    let range_elapsed = range_started.elapsed().as_millis();
    proxy_reader
        .close()
        .await
        .expect("close ProxyCommand range reader");
    emit_transport_metric("proxy_300_range_reads", RANGE_READ_SAMPLES, range_elapsed);

    let concurrent_started = Instant::now();
    let (direct_a, direct_b, proxy_a, proxy_b) = tokio::join!(
        open_read_close(&manager, "m7-perf-direct", &remote_small_file),
        open_read_close(&manager, "m7-perf-direct", &remote_small_file),
        open_read_close(&manager, "m7-perf-proxy", &remote_small_file),
        open_read_close(&manager, "m7-perf-proxy", &remote_small_file),
    );
    for result in [direct_a, direct_b, proxy_a, proxy_b] {
        result.expect("mixed Direct+Proxy concurrent reader");
    }
    emit_transport_metric(
        "direct2_proxy2_concurrent",
        4,
        concurrent_started.elapsed().as_millis(),
    );
}

async fn measure_setup(manager: &SshConnectionManager, id: &str, path: &str) -> u128 {
    let started = Instant::now();
    for _ in 0..SETUP_SAMPLES {
        open_read_close(manager, id, path)
            .await
            .expect("transport setup sample");
    }
    started.elapsed().as_millis()
}

async fn open_read_close(
    manager: &SshConnectionManager,
    connection_id: &str,
    path: &str,
) -> Result<(), log_query_mcp::transport::SshTransportError> {
    let reader = manager.open_reader(connection_id).await?;
    let bytes = reader.read_range(path, 6, 5).await?;
    assert_eq!(bytes, b"world");
    reader.close().await
}

fn prepare_fixture(path: &Path, logical_size: u64, dense_start: u64, marker: &[u8]) {
    assert!(dense_start <= logical_size);
    let dense_bytes = logical_size - dense_start;
    assert!(dense_bytes >= marker.len() as u64);

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create M7 performance fixture directory");
    }
    let file = File::create(path).expect("create M7 performance fixture");
    file.set_len(dense_start)
        .expect("create sparse M7 benchmark prefix");
    let mut writer = BufWriter::with_capacity(FILL_BUFFER_BYTES, file);
    writer
        .seek(SeekFrom::Start(dense_start))
        .expect("seek M7 benchmark dense range");
    write_dense_region(&mut writer, dense_bytes, marker);
    writer.flush().expect("flush M7 performance fixture");
}

fn append_fixture(path: &Path, bytes: u64, marker: &[u8]) {
    assert!(bytes >= marker.len() as u64);
    let file = OpenOptions::new()
        .append(true)
        .open(path)
        .expect("open M7 benchmark fixture for append");
    let mut writer = BufWriter::with_capacity(FILL_BUFFER_BYTES, file);
    write_dense_region(&mut writer, bytes, marker);
    writer.flush().expect("flush M7 benchmark append");
}

fn write_dense_region<W: Write>(writer: &mut W, bytes: u64, marker: &[u8]) {
    let filler_bytes = bytes - marker.len() as u64;
    let line = b"M7_PERF_FILLER 0123456789abcdef0123456789abcdef0123456789abcdef\n";
    let mut block = Vec::with_capacity(FILL_BUFFER_BYTES);
    while block.len() + line.len() <= FILL_BUFFER_BYTES {
        block.extend_from_slice(line);
    }

    let mut remaining = filler_bytes;
    while remaining > 0 {
        let chunk = remaining.min(block.len() as u64) as usize;
        writer
            .write_all(&block[..chunk])
            .expect("write M7 benchmark filler");
        remaining -= chunk as u64;
    }
    writer.write_all(marker).expect("write M7 benchmark marker");
}

#[allow(clippy::too_many_arguments)]
fn emit_sync_metric(
    transport: &str,
    profile: &str,
    scenario: &str,
    remote_size: u64,
    elapsed_ms: u128,
    remote_bytes_read: u64,
    cached_bytes_written: u64,
    cache_disk_bytes: u64,
    bytes_scanned: Option<u64>,
) {
    let value = json!({
        "transport": transport,
        "profile": profile,
        "scenario": scenario,
        "remote_size_bytes": remote_size,
        "elapsed_ms": elapsed_ms,
        "remote_bytes_read": remote_bytes_read,
        "cached_bytes_written": cached_bytes_written,
        "cache_disk_bytes": cache_disk_bytes,
        "bytes_scanned": bytes_scanned,
    });
    println!("M7_PERF_METRIC {value}");
}

fn emit_transport_metric(scenario: &str, samples: usize, elapsed_ms: u128) {
    let value = json!({
        "scenario": scenario,
        "samples": samples,
        "elapsed_ms": elapsed_ms,
    });
    println!("M7_TRANSPORT_PERF_METRIC {value}");
}

fn cache_disk_bytes(root: &Path) -> u64 {
    if !root.exists() {
        return 0;
    }
    let mut total = 0_u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        for entry in fs::read_dir(directory).expect("read M7 benchmark cache directory") {
            let path = entry.expect("M7 benchmark cache entry").path();
            let metadata = fs::symlink_metadata(&path).expect("M7 benchmark cache metadata");
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                total = total.saturating_add(metadata.len());
            }
        }
    }
    total
}
