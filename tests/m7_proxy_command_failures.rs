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

fn proxy_connection_with_secret(
    connection_id: &str,
    program: &str,
    args: Vec<String>,
    secret_ref: &str,
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
            "secret_ref": secret_ref
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

fn proxy_connection(
    connection_id: &str,
    program: &str,
    args: Vec<String>,
    connect_timeout_millis: u64,
) -> Value {
    proxy_connection_with_secret(
        connection_id,
        program,
        args,
        "M2_SSH_PASSWORD",
        connect_timeout_millis,
    )
}

fn direct_connection(connection_id: &str, connect_timeout_millis: u64) -> Value {
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
        "connect_timeout_millis": connect_timeout_millis,
        "operation_timeout_millis": 3000,
        "keepalive_seconds": 5
    })
}

fn config_with_limit(
    connections: Vec<Value>,
    source_connection_id: &str,
    max_concurrent_ssh_connections: usize,
) -> AppConfigV2 {
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
            "max_concurrent_ssh_connections": max_concurrent_ssh_connections
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("M7 failure config should be valid")
}

fn config(connections: Vec<Value>, source_connection_id: &str) -> AppConfigV2 {
    config_with_limit(connections, source_connection_id, 1)
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
    for _ in 0..150 {
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

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn wrong_password_through_proxy_preserves_authentication_error_and_reaps_child() {
    let pid_file = required("M7_AUTH_PID_FILE");
    let trigger_file = required("M7_AUTH_TRIGGER_FILE");
    let _ = fs::remove_file(&pid_file);
    let _ = fs::remove_file(&trigger_file);
    let connection = proxy_connection_with_secret(
        "bad-auth",
        &required("M7_CONTROLLED_PROXY_PROGRAM"),
        vec![
            "{host}".to_owned(),
            "{port}".to_owned(),
            trigger_file,
            pid_file.clone(),
        ],
        "M7_BAD_SSH_PASSWORD",
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "bad-auth"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("bad-auth")
            .await
            .expect_err("wrong password through proxy must remain an SSH auth failure"),
        SshTransportError::AuthenticationFailed
    );
    let pid = wait_for_pid(&pid_file).await;
    wait_for_process_exit(pid).await;
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn active_proxy_crash_breaks_sftp_reader_fail_closed() {
    let pid_file = required("M7_CRASH_PID_FILE");
    let trigger_file = required("M7_CRASH_TRIGGER_FILE");
    let remote_file = required("M2_REMOTE_FILE");
    let _ = fs::remove_file(&pid_file);
    let _ = fs::remove_file(&trigger_file);
    let connection = proxy_connection(
        "active-crash",
        &required("M7_CONTROLLED_PROXY_PROGRAM"),
        vec![
            "{host}".to_owned(),
            "{port}".to_owned(),
            trigger_file.clone(),
            pid_file.clone(),
        ],
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "active-crash"))
        .expect("connection manager should build");
    let reader = manager
        .open_reader("active-crash")
        .await
        .expect("controlled proxy must establish SSH/SFTP before the injected crash");

    reader
        .stat(&remote_file)
        .await
        .expect("SFTP should work before the proxy crashes");
    let pid = wait_for_pid(&pid_file).await;
    fs::write(&trigger_file, b"crash\n").expect("should trigger controlled proxy crash");
    wait_for_process_exit(pid).await;

    let first_error = reader
        .stat(&remote_file)
        .await
        .expect_err("active proxy crash must fail the next SFTP operation");
    assert!(matches!(
        first_error,
        SshTransportError::SftpProtocol
            | SshTransportError::OperationTimeout
            | SshTransportError::Broken
    ));
    assert_eq!(
        reader
            .stat(&remote_file)
            .await
            .expect_err("reader must remain fail-closed after the transport breaks"),
        SshTransportError::Broken
    );
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand failure workflow fixture"]
async fn stalled_proxy_does_not_break_active_direct_transport() {
    let pid_file = required("M7_MIXED_PID_FILE");
    let remote_file = required("M2_REMOTE_FILE");
    let _ = fs::remove_file(&pid_file);
    let stall = proxy_connection(
        "proxy-stall",
        &required("M7_STALL_PROGRAM"),
        vec![pid_file.clone()],
        5000,
    );
    let direct = direct_connection("direct-good", 3000);
    let manager = SshConnectionManager::from_config(&config_with_limit(
        vec![stall, direct],
        "direct-good",
        2,
    ))
    .expect("connection manager should build");

    let proxy_manager = manager.clone();
    let proxy_task = tokio::spawn(async move { proxy_manager.open_reader("proxy-stall").await });
    let pid = wait_for_pid(&pid_file).await;

    let direct_reader = manager
        .open_reader("direct-good")
        .await
        .expect("a stalled ProxyCommand must not prevent an independent Direct connection");
    direct_reader
        .stat(&remote_file)
        .await
        .expect("Direct SFTP must work while ProxyCommand is stalled");

    proxy_task.abort();
    let _ = proxy_task.await;
    wait_for_process_exit(pid).await;

    direct_reader
        .stat(&remote_file)
        .await
        .expect("cancelling the ProxyCommand path must not break the active Direct session");
    direct_reader
        .close()
        .await
        .expect("Direct reader should close cleanly");
}
