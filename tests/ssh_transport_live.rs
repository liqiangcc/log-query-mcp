#![cfg(target_os = "linux")]

use std::{env, time::Duration};

use log_query_mcp::{
    AppConfigV2,
    transport::{RemoteFileType, SshConnectionManager, SshTransportError},
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

fn password_connection(
    connection_id: &str,
    ssh_port: u16,
    known_hosts: &str,
    secret_ref: &str,
    connect_timeout_millis: u64,
    operation_timeout_millis: u64,
) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": ssh_port,
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": secret_ref
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "connect_timeout_millis": connect_timeout_millis,
        "operation_timeout_millis": operation_timeout_millis,
        "keepalive_seconds": 5
    })
}

fn private_key_connection(
    connection_id: &str,
    ssh_port: u16,
    known_hosts: &str,
    key_file: &str,
) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": ssh_port,
        "username": "logreader",
        "auth": {
            "type": "private_key",
            "key_file": key_file,
            "passphrase_secret_ref": "M2_KEY_PASSPHRASE"
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "connect_timeout_millis": 3000,
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
            "source_id": "m2-live-source",
            "name": "M2 live source",
            "service": "m2-live",
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
            "root": "/tmp/log-query-mcp-m2-cache",
            "max_bytes": 1048576,
            "max_bytes_per_source": 1048576,
            "retention_hours": 1,
            "max_generations_per_file": 2
        },
        "limits": {
            "max_concurrent_ssh_connections": 1
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("live test config should be valid")
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn password_auth_supports_read_only_sftp_operations() {
    let ssh_port = port("M2_SSH_PORT");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let connection = password_connection(
        "good",
        ssh_port,
        &known_hosts,
        "M2_SSH_PASSWORD",
        3000,
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "good"))
        .expect("connection manager should build");
    let reader = manager
        .open_reader("good")
        .await
        .expect("password SSH reader should connect");

    let remote_file = required("M2_REMOTE_FILE");
    let remote_dir = required("M2_REMOTE_DIR");
    let metadata = reader
        .stat(&remote_file)
        .await
        .expect("stat should succeed");
    assert_eq!(metadata.file_type, RemoteFileType::Regular);
    let lstat = reader
        .lstat(&remote_file)
        .await
        .expect("lstat should succeed");
    assert_eq!(lstat.file_type, RemoteFileType::Regular);
    let entries = reader
        .read_dir(&remote_dir)
        .await
        .expect("readdir should succeed");
    assert!(
        entries
            .iter()
            .any(|entry| entry.file_name == "application.log")
    );
    let range = reader
        .read_range(&remote_file, 6, 5)
        .await
        .expect("range read should succeed");
    assert_eq!(range, b"world");

    reader.close().await.expect("SFTP session should close");
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn encrypted_private_key_authentication_reads_logs() {
    let connection = private_key_connection(
        "key",
        port("M2_SSH_PORT"),
        &required("M2_KNOWN_HOSTS"),
        &required("M2_SSH_KEY_FILE"),
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "key"))
        .expect("connection manager should build");
    let reader = manager
        .open_reader("key")
        .await
        .expect("private key SSH reader should connect");

    assert_eq!(
        reader
            .read_range(&required("M2_REMOTE_FILE"), 6, 5)
            .await
            .expect("range read should succeed"),
        b"world"
    );
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn wrong_password_is_rejected_without_leaking_secret() {
    let connection = password_connection(
        "bad-password",
        port("M2_SSH_PORT"),
        &required("M2_KNOWN_HOSTS"),
        "M2_WRONG_PASSWORD",
        3000,
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "bad-password"))
        .expect("connection manager should build");

    let error = manager
        .open_reader("bad-password")
        .await
        .expect_err("wrong password must fail");
    assert_eq!(error, SshTransportError::AuthenticationFailed);
    assert!(!error.to_string().contains(&required("M2_WRONG_PASSWORD")));
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn changed_host_key_fails_closed() {
    let connection = password_connection(
        "bad-host-key",
        port("M2_SSH_PORT"),
        &required("M2_BAD_KNOWN_HOSTS"),
        "M2_SSH_PASSWORD",
        3000,
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "bad-host-key"))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("bad-host-key")
            .await
            .expect_err("changed host key must fail"),
        SshTransportError::HostKeyVerificationFailed
    );
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn permission_and_missing_file_fail_without_exposing_paths() {
    let connection = password_connection(
        "good",
        port("M2_SSH_PORT"),
        &required("M2_KNOWN_HOSTS"),
        "M2_SSH_PASSWORD",
        3000,
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "good"))
        .expect("connection manager should build");

    let reader = manager
        .open_reader("good")
        .await
        .expect("reader should connect");
    let denied = required("M2_DENIED_FILE");
    let error = reader
        .read_range(&denied, 0, 1)
        .await
        .expect_err("permission denied file must fail");
    assert!(!error.to_string().contains(&denied));
    drop(reader);

    let reader = manager
        .open_reader("good")
        .await
        .expect("new reader should connect after broken reader is dropped");
    let missing = format!("{}/missing.log", required("M2_REMOTE_DIR"));
    let error = reader
        .stat(&missing)
        .await
        .expect_err("missing file must fail");
    assert!(!error.to_string().contains(&missing));
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn operation_timeout_marks_reader_broken() {
    let connection = password_connection(
        "timeout",
        port("M2_SSH_PORT"),
        &required("M2_KNOWN_HOSTS"),
        "M2_SSH_PASSWORD",
        3000,
        300,
    );
    let manager = SshConnectionManager::from_config(&config(vec![connection], "timeout"))
        .expect("connection manager should build");
    let reader = manager
        .open_reader("timeout")
        .await
        .expect("reader should connect");

    assert_eq!(
        reader
            .read_range(&required("M2_FIFO_FILE"), 0, 1)
            .await
            .expect_err("blocking FIFO read must time out"),
        SshTransportError::OperationTimeout
    );
    assert_eq!(
        reader
            .stat(&required("M2_REMOTE_FILE"))
            .await
            .expect_err("timed out reader must remain broken"),
        SshTransportError::Broken
    );
}

#[tokio::test]
#[ignore = "requires the SSH Transport workflow fixture"]
async fn cancelling_connect_releases_global_connection_permit() {
    let good_port = port("M2_SSH_PORT");
    let stall_port = port("M2_STALL_PORT");
    let known_hosts = required("M2_KNOWN_HOSTS");
    let stall = password_connection(
        "stall",
        stall_port,
        &known_hosts,
        "M2_SSH_PASSWORD",
        5000,
        3000,
    );
    let good = password_connection(
        "good",
        good_port,
        &known_hosts,
        "M2_SSH_PASSWORD",
        3000,
        3000,
    );
    let manager = SshConnectionManager::from_config(&config(vec![stall, good], "good"))
        .expect("connection manager should build");

    let stalled_manager = manager.clone();
    let task = tokio::spawn(async move { stalled_manager.open_reader("stall").await });
    sleep(Duration::from_millis(100)).await;
    task.abort();
    let _ = task.await;

    manager
        .open_reader("good")
        .await
        .expect("cancelling connect must release the semaphore permit");
}
