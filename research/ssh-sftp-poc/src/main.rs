use std::env;
use std::io::SeekFrom;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use russh::client;
use russh::keys::{self, PrivateKeyWithHashAlg};
use russh_sftp::client::SftpSession;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio::time::timeout;

#[derive(Clone)]
struct KnownHostsClient {
    host: String,
    port: u16,
    known_hosts: PathBuf,
}

impl client::Handler for KnownHostsClient {
    type Error = anyhow::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let matched = keys::known_hosts::check_known_hosts_path(
            &self.host,
            self.port,
            server_public_key,
            &self.known_hosts,
        )
        .context("known_hosts verification failed")?;
        Ok(matched)
    }
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("missing environment variable {name}"))
}

fn parse_u16(name: &str, default: u16) -> Result<u16> {
    Ok(env::var(name)
        .ok()
        .map(|value| value.parse())
        .transpose()
        .with_context(|| format!("invalid {name}"))?
        .unwrap_or(default))
}

fn parse_u64(name: &str, default: u64) -> Result<u64> {
    Ok(env::var(name)
        .ok()
        .map(|value| value.parse())
        .transpose()
        .with_context(|| format!("invalid {name}"))?
        .unwrap_or(default))
}

async fn connect(
    host: &str,
    port: u16,
    known_hosts: PathBuf,
    connect_timeout: Duration,
) -> Result<client::Handle<KnownHostsClient>> {
    let config = client::Config {
        inactivity_timeout: Some(Duration::from_secs(30)),
        keepalive_interval: Some(Duration::from_secs(10)),
        keepalive_max: 2,
        ..Default::default()
    };
    let handler = KnownHostsClient {
        host: host.to_owned(),
        port,
        known_hosts,
    };

    timeout(
        connect_timeout,
        client::connect(Arc::new(config), (host, port), handler),
    )
    .await
    .context("SSH connect timeout")?
    .context("SSH connect failed")
}

async fn authenticate_password(
    session: &mut client::Handle<KnownHostsClient>,
    username: &str,
    password: String,
    operation_timeout: Duration,
) -> Result<()> {
    let result = timeout(
        operation_timeout,
        session.authenticate_password(username, password),
    )
    .await
    .context("password authentication timeout")??;
    if !result.success() {
        bail!("password authentication rejected");
    }
    Ok(())
}

async fn authenticate_key(
    session: &mut client::Handle<KnownHostsClient>,
    username: &str,
    key_file: PathBuf,
    passphrase: String,
    operation_timeout: Duration,
) -> Result<()> {
    let key = tokio::task::spawn_blocking(move || {
        keys::load_secret_key(key_file, Some(passphrase.as_str()))
    })
    .await
    .context("private key loader task failed")?
    .context("private key load/decrypt failed")?;

    let hash = timeout(operation_timeout, session.best_supported_rsa_hash())
        .await
        .context("RSA hash negotiation timeout")??
        .flatten();
    let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
    let result = timeout(
        operation_timeout,
        session.authenticate_publickey(username, key),
    )
    .await
    .context("public key authentication timeout")??;
    if !result.success() {
        bail!("public key authentication rejected");
    }
    Ok(())
}

async fn verify_sftp(
    session: &mut client::Handle<KnownHostsClient>,
    remote_dir: &str,
    remote_file: &str,
    operation_timeout: Duration,
) -> Result<()> {
    let channel = timeout(operation_timeout, session.channel_open_session())
        .await
        .context("open SSH channel timeout")??;
    timeout(
        operation_timeout,
        channel.request_subsystem(true, "sftp"),
    )
    .await
    .context("SFTP subsystem request timeout")??;

    let sftp = timeout(operation_timeout, SftpSession::new(channel.into_stream()))
        .await
        .context("SFTP initialization timeout")??;

    timeout(operation_timeout, sftp.metadata(remote_file))
        .await
        .context("SFTP stat timeout")??;
    timeout(operation_timeout, sftp.symlink_metadata(remote_file))
        .await
        .context("SFTP lstat timeout")??;

    let entries = timeout(operation_timeout, sftp.read_dir(remote_dir))
        .await
        .context("SFTP readdir timeout")??;
    let file_name = remote_file.rsplit('/').next().unwrap_or(remote_file);
    if !entries.into_iter().any(|entry| entry.file_name() == file_name) {
        bail!("remote test file was not returned by readdir");
    }

    let mut file = timeout(operation_timeout, sftp.open(remote_file))
        .await
        .context("SFTP open timeout")??;
    timeout(operation_timeout, file.seek(SeekFrom::Start(6)))
        .await
        .context("SFTP seek timeout")??;

    let mut buffer = [0_u8; 5];
    timeout(operation_timeout, file.read_exact(&mut buffer))
        .await
        .context("SFTP range read timeout")??;
    if &buffer != b"world" {
        bail!("unexpected range read contents: {buffer:?}");
    }

    timeout(operation_timeout, sftp.close())
        .await
        .context("SFTP close timeout")??;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let host = required("POC_SSH_HOST")?;
    let port = parse_u16("POC_SSH_PORT", 22)?;
    let username = required("POC_SSH_USERNAME")?;
    let known_hosts = PathBuf::from(required("POC_KNOWN_HOSTS")?);
    let auth = required("POC_AUTH")?;
    let remote_dir = required("POC_REMOTE_DIR")?;
    let remote_file = required("POC_REMOTE_FILE")?;
    let connect_timeout = Duration::from_millis(parse_u64("POC_CONNECT_TIMEOUT_MS", 3000)?);
    let operation_timeout = Duration::from_millis(parse_u64("POC_OPERATION_TIMEOUT_MS", 3000)?);

    let mut session = connect(&host, port, known_hosts, connect_timeout).await?;
    match auth.as_str() {
        "password" => {
            authenticate_password(
                &mut session,
                &username,
                required("POC_SSH_PASSWORD")?,
                operation_timeout,
            )
            .await?;
        }
        "key" => {
            authenticate_key(
                &mut session,
                &username,
                PathBuf::from(required("POC_SSH_KEY_FILE")?),
                required("POC_SSH_KEY_PASSPHRASE")?,
                operation_timeout,
            )
            .await?;
        }
        other => bail!("unsupported POC_AUTH={other}"),
    }

    verify_sftp(
        &mut session,
        &remote_dir,
        &remote_file,
        operation_timeout,
    )
    .await?;

    println!("SSH/SFTP POC succeeded using {auth} authentication");
    Ok(())
}
