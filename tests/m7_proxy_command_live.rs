#![cfg(target_os = "linux")]

use std::env;

use log_query_mcp::{
    AppConfigV2,
    transport::{SshConnectionManager, SshTransportError},
};
use serde_json::{Value, json};

fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("missing test environment variable {name}"))
}

fn port(name: &str) -> u16 {
    required(name)
        .parse()
        .unwrap_or_else(|_| panic!("invalid test port in {name}"))
}

fn proxy_password_connection(
    connection_id: &str,
    ssh_port: u16,
    known_hosts: &str,
    proxy_program: &str,
) -> Value {
    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": ssh_port,
        "username": "logreader",
        "auth": {
            "type": "password",
            "secret_ref": "M2_SSH_PASSWORD"
        },
        "host_key": {
            "known_hosts_file": known_hosts
        },
        "proxy": {
            "type": "command",
            "program": proxy_program,
            "args": ["{host}", "{port}"]
        },
        "connect_timeout_millis": 3000,
        "operation_timeout_millis": 3000,
        "keepalive_seconds": 5
    })
}

fn config(connection: Value) -> AppConfigV2 {
    let remote_dir = required("M2_REMOTE_DIR");
    let remote_file = required("M2_REMOTE_FILE");
    let file_name = remote_file
        .rsplit('/')
        .next()
        .expect("remote file should have a file name");
    let document = json!({
        "version": 2,
        "connections": [connection],
        "sources": [{
            "source_id": "m7-proxy-live-source",
            "name": "M7 ProxyCommand live source",
            "service": "m7-proxy-live",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": "proxy"
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
            "root": "/tmp/log-query-mcp-m7-proxy-cache",
            "max_bytes": 1048576,
            "max_bytes_per_source": 1048576,
            "retention_hours": 1,
            "max_generations_per_file": 2
        },
        "limits": {
            "max_concurrent_ssh_connections": 1
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("M7 live config should be valid")
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand workflow fixture"]
async fn proxy_command_reaches_openssh_and_reads_sftp() {
    let connection = proxy_password_connection(
        "proxy",
        port("M2_SSH_PORT"),
        &required("M2_KNOWN_HOSTS"),
        &required("M7_PROXY_PROGRAM"),
    );
    let manager = SshConnectionManager::from_config(&config(connection))
        .expect("connection manager should build");
    let reader = manager
        .open_reader("proxy")
        .await
        .expect("ProxyCommand SSH reader should connect");

    let bytes = reader
        .read_range(&required("M2_REMOTE_FILE"), 6, 5)
        .await
        .expect("ProxyCommand SFTP range read should succeed");
    assert_eq!(bytes, b"world");

    reader.close().await.expect("SFTP session should close");
}

#[tokio::test]
#[ignore = "requires the M7 ProxyCommand workflow fixture"]
async fn proxy_command_does_not_bypass_strict_host_key_verification() {
    let connection = proxy_password_connection(
        "proxy",
        port("M2_SSH_PORT"),
        &required("M2_BAD_KNOWN_HOSTS"),
        &required("M7_PROXY_PROGRAM"),
    );
    let manager = SshConnectionManager::from_config(&config(connection))
        .expect("connection manager should build");

    assert_eq!(
        manager
            .open_reader("proxy")
            .await
            .expect_err("wrong host key must fail through ProxyCommand"),
        SshTransportError::HostKeyVerificationFailed
    );
}
