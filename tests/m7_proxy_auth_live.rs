#![cfg(target_os = "linux")]

use log_query_mcp::{
    AppConfigV2,
    transport::{RemoteFileType, SshConnectionManager},
};
use serde_json::{Value, json};

fn required(name: &str) -> String {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("missing required test environment variable {name}"))
}

fn port() -> u16 {
    required("M7_AUTH_SSH_PORT")
        .parse()
        .expect("valid M7 auth SSH port")
}

fn proxy_key_connection(
    connection_id: &str,
    key_file: &str,
    passphrase_secret_ref: Option<&str>,
) -> Value {
    let mut auth = json!({
        "type": "private_key",
        "key_file": key_file
    });
    if let Some(secret_ref) = passphrase_secret_ref {
        auth["passphrase_secret_ref"] = json!(secret_ref);
    }

    json!({
        "connection_id": connection_id,
        "type": "ssh",
        "host": "127.0.0.1",
        "port": port(),
        "username": "logreader",
        "auth": auth,
        "host_key": {
            "known_hosts_file": required("M7_AUTH_KNOWN_HOSTS")
        },
        "proxy": {
            "type": "command",
            "program": required("M7_AUTH_PROXY_PROGRAM"),
            "args": ["{host}", "{port}"]
        },
        "connect_timeout_millis": 3000,
        "operation_timeout_millis": 5000,
        "keepalive_seconds": 5
    })
}

fn config(connection: Value, connection_id: &str) -> AppConfigV2 {
    let remote_file = required("M7_AUTH_REMOTE_FILE");
    let file_name = remote_file
        .rsplit('/')
        .next()
        .expect("remote file should have a file name");
    let document = json!({
        "version": 2,
        "connections": [connection],
        "sources": [{
            "source_id": "m7-proxy-auth-source",
            "name": "M7 Proxy auth source",
            "service": "m7-proxy-auth",
            "environment": "test",
            "backend": {
                "type": "ssh",
                "connection_id": connection_id
            },
            "root": required("M7_AUTH_REMOTE_DIR"),
            "files": [file_name],
            "sync": {
                "freshness": "on_query",
                "bootstrap": {"type": "full"},
                "allow_stale_on_error": false
            }
        }],
        "cache": {
            "root": "/tmp/log-query-mcp-m7-auth-cache",
            "max_bytes": 1048576,
            "max_bytes_per_source": 1048576,
            "retention_hours": 1,
            "max_generations_per_file": 2
        },
        "limits": {
            "max_concurrent_ssh_connections": 1
        }
    });

    AppConfigV2::from_json_str(&document.to_string()).expect("valid M7 Proxy auth config")
}

async fn assert_key_auth_reads(connection: Value, connection_id: &str) {
    let manager = SshConnectionManager::from_config(&config(connection, connection_id))
        .expect("M7 Proxy auth manager should build");
    let reader = manager
        .open_reader(connection_id)
        .await
        .expect("ProxyCommand private-key SSH reader should connect");

    let remote_file = required("M7_AUTH_REMOTE_FILE");
    let metadata = reader
        .stat(&remote_file)
        .await
        .expect("ProxyCommand key-auth stat should succeed");
    assert_eq!(metadata.file_type, RemoteFileType::Regular);
    assert_eq!(
        reader
            .read_range(&remote_file, 6, 5)
            .await
            .expect("ProxyCommand key-auth range read should succeed"),
        b"world"
    );
    reader.close().await.expect("key-auth SFTP should close");
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Auth OpenSSH fixture"]
async fn unencrypted_private_key_authentication_works_through_proxy_command() {
    let connection = proxy_key_connection(
        "proxy-unencrypted-key",
        &required("M7_AUTH_UNENCRYPTED_KEY_FILE"),
        None,
    );
    assert_key_auth_reads(connection, "proxy-unencrypted-key").await;
}

#[tokio::test]
#[ignore = "requires the M7 Proxy Auth OpenSSH fixture"]
async fn encrypted_private_key_with_passphrase_works_through_proxy_command() {
    let connection = proxy_key_connection(
        "proxy-encrypted-key",
        &required("M7_AUTH_ENCRYPTED_KEY_FILE"),
        Some("M7_AUTH_KEY_PASSPHRASE"),
    );
    assert_key_auth_reads(connection, "proxy-encrypted-key").await;
}
