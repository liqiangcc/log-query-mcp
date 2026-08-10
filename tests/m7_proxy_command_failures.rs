#![cfg(target_os = "linux")]

use std::{env, fs, path::Path, time::Duration};

use log_query_mcp::{
    AppConfigV2,
    transport::{SshConnectionManager, SshTransportError},
};
use serde_json::{Value, json};
use tokio::time::sleep;

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("missing test environment variable {name}"))
}

fn port(name: &str) -> u16 {
    required(name)
        .parse()
        .unwrap_or_else(|_| panic!("invalid test port in {name}"))
}

fn proxy_connection(
    connection_id: &str,
    program: &str,
    args: Vec<String>,
    connect_timeout_millis: u64,
) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port("M2_SSH_PORT"),
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M2_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": required("M2_KNOWN_HOSTS")
        },
        "proxy": {
            "type": "command",
            "program": program,
            "args": args
        },
        "connect_timeout_millis": connect_timeout_millis,
        "operation_timeout_millis": 3000,
        "keepalive_seconds": 5
    })
}

fn config(connections: Vec<Value>, source_connection_id: &str) -> AppConfigV2 {
    let remote_dir = required("M2_REMOTE_DIR");
    let remote_file = required("M2_REMOTE_FILE");
    let file_name = remote_file
        .rsplit('/')
        .next()
        .expect("remote file should have a file name");
    let document = json!({
        "version": 2,
        "connections": connections,
        "sources": [{
            "source_id": "m7-proxy-failure-source",
            "name": "M7 ProxyCommand failure source",
            "service": "m7-proxy-failure",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": source_connection_id
            },
            "root": remote_dir,
            "files": [file_name],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": "/tmp/log-query-mcp-m7-proxy-failure-cache",
            "max_bytes": 1048576,
            "max_bytes_per_source": 1048576,
            "retention_hours": 1,
            "max_generations_per_file": 2
        },
        "limits": {
            "max_concurrent_ssh_connections": 1
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("M7 failure config should be valid")
}

async fn wait_for_pid(path: &str) -> u32 {
    for _ in 0..100 {
        if let Ok(value) = fs::read_to_string(path) {
            return value
                .trim()
                .parse()
                .expect("proxy helper should write a numeric pid");
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("proxy helper did not write pid file {path}");
}

async fn wait_for_process_exit(pid: u32) {
    let proc_path = format!("/proc/{pid}");
    for _ in 0..100 {
        if !Path::new(&proc_path).exists() {
            return;
        }
        sleep(Duration::from_millis(20)).await;
    }
    panic!("proxy helper process {pid} was not reaped");
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn missing_proxy_program_has_stable_classification() {
    let connection = proxy_connection(
        "missing",
        "/definitely/missing/log-query-mcp-proxy-helper",
        Vec::new(),
        1000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "missing"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("missing")
            .await
            .expect_err("missing helper must fail"),
        SshTransportError::ProxyCommandNotFound
    );
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn non_executable_proxy_program_has_stable_classification() {
    let connection = proxy_connection(
        "nonexec",
        &required("M7_NONEXEC_PROGRAM"),
        Vec::new(),
        1000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "nonexec"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("nonexec")
            .await
            .expect_err("non-executable helper must fail"),
        SshTransportError::ProxyCommandPermissionDenied
    );
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn early_proxy_exit_is_classified_as_stream_failure() {
    let connection = proxy_connection("early-exit", "/usr/bin/false", Vec::new(), 1000);
    let manager = SshConnectionManager::from_config(&config(vec![connection], "early-exit"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("early-exit")
            .await
            .expect_err("early proxy exit must fail SSH stream establishment"),
        SshTransportError::ProxyCommandStreamFailed
    );
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn stderr_flood_still_obeys_proxy_connect_timeout_and_reaps_child() {
    let pid_file = required("M7_TIMEOUT_PID_FILE");
    let _ = fs::remove_file(&pid_file);
    let connection = proxy_connection(
        "timeout",
        &required("M7_STALL_PROGRAM"),
        vec![pid_file.clone()],
        300,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "timeout"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("timeout")
            .await
            .expect_err("stalling proxy must hit the proxy connect deadline"),
        SshTransportError::ProxyCommandTimeout
    );
    let pid = wait_for_pid(&pid_file).await;
    wait_for_process_exit(pid).await;
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn cancelling_proxy_connect_reaps_child_and_releases_global_permit() {
    let pid_file = required("M7_CANCEL_PID_FILE");
    let _ = fs::remove_file(&pid_file);
    let stall = proxy_connection(
        "stall",
        &required("M7_STALL_PROGRAM"),
        vec![pid_file.clone()],
        5000,
    );
    let good = proxy_connection(
        "good",
        &required("M7_PROXY_PROGRAM"),
        vec!["{host}".to_owned(), "{port}".to_owned()],
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![stall, good], "good"))
        .expect("connection manager should build");

    let stalled_manager = manager.clone();
    let task = tokio::spawn(async move { stalled_manager.open_reader("stall").await });
    let pid = wait_for_pid(&pid_file).await;
    task.abort();
    let _ = task.await;
    wait_for_process_exit(pid).await;

    let reader = manager
        .open_reader("good")
        .await
        .expect("cancellation must release the shared SSH connection permit");
    reader.close().await.expect("good reader should close");
}
