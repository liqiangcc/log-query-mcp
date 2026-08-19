use std::{
    collections::HashMap,
    fmt,
    io::SeekFrom,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use russh::keys::PrivateKeyWithHashAlg;
use russh::{client, keys};
use russh_sftp::{client::SftpSession, protocol::FileAttributes};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
    sync::{OwnedSemaphorePermit, Semaphore},
    task,
    time::timeout,
};

use crate::{AppConfigV2, EnvSecretResolver, SecretResolver, SshAuthType, SshConnectionConfig};

use super::proxy_command::{ProxyCommandConnectError, connect_proxy_command};

pub const MAX_READ_RANGE_BYTES: usize = 4 * 1024 * 1024;
const S_IFMT: u32 = 0o170000;
const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;

trait SshIoStream: AsyncRead + AsyncWrite + Unpin + Send {}

impl<T> SshIoStream for T where T: AsyncRead + AsyncWrite + Unpin + Send {}

type BoxedSshStream = Box<dyn SshIoStream>;

#[derive(Debug, Clone, Copy, Default)]
struct SshStreamConnector;

impl SshStreamConnector {
    async fn connect(
        &self,
        connection: &SshConnectionConfig,
    ) -> Result<BoxedSshStream, SshTransportError> {
        if connection.proxy.is_some() {
            let stream = connect_proxy_command(connection).map_err(map_proxy_connect_error)?;
            return Ok(Box::new(stream));
        }
        DirectConnector.connect(connection).await
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct DirectConnector;

impl DirectConnector {
    async fn connect(
        &self,
        connection: &SshConnectionConfig,
    ) -> Result<BoxedSshStream, SshTransportError> {
        let stream = TcpStream::connect((connection.host.as_str(), connection.port))
            .await
            .map_err(|_| SshTransportError::ConnectFailed)?;
        Ok(Box::new(stream))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteFileType {
    Regular,
    Directory,
    Symlink,
    Other,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteFileMetadata {
    pub size: Option<u64>,
    pub permissions: Option<u32>,
    pub mtime: Option<u32>,
    pub file_type: RemoteFileType,
}

impl From<FileAttributes> for RemoteFileMetadata {
    fn from(attributes: FileAttributes) -> Self {
        Self {
            size: attributes.size,
            permissions: attributes.permissions,
            mtime: attributes.mtime,
            file_type: classify_file_type(attributes.permissions),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteDirEntry {
    pub file_name: String,
}

#[derive(Clone)]
pub struct SshConnectionManager {
    connections: Arc<HashMap<String, SshConnectionConfig>>,
    secrets: Arc<dyn SecretResolver>,
    permits: Arc<Semaphore>,
}

impl fmt::Debug for SshConnectionManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshConnectionManager")
            .field("connection_count", &self.connections.len())
            .field("available_permits", &self.permits.available_permits())
            .finish()
    }
}

impl SshConnectionManager {
    pub fn from_config(config: &AppConfigV2) -> Result<Self, SshTransportError> {
        Self::new(config, Arc::new(EnvSecretResolver))
    }

    pub fn new(
        config: &AppConfigV2,
        secrets: Arc<dyn SecretResolver>,
    ) -> Result<Self, SshTransportError> {
        let max_connections = config.limits.max_concurrent_ssh_connections;
        if max_connections == 0 {
            return Err(SshTransportError::InvalidConfiguration);
        }

        let connections = config
            .connections
            .iter()
            .cloned()
            .map(|connection| (connection.connection_id.clone(), connection))
            .collect();

        Ok(Self {
            connections: Arc::new(connections),
            secrets,
            permits: Arc::new(Semaphore::new(max_connections)),
        })
    }

    pub async fn open_reader(
        &self,
        connection_id: &str,
    ) -> Result<SshReadTransport, SshTransportError> {
        let connection = self
            .connections
            .get(connection_id)
            .cloned()
            .ok_or(SshTransportError::UnknownConnection)?;
        let connect_timeout = Duration::from_millis(connection.connect_timeout_millis);
        let permit = timeout(connect_timeout, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| SshTransportError::ConnectionLimit)?
            .map_err(|_| SshTransportError::ConnectionManagerClosed)?;

        SshReadTransport::connect(connection, self.secrets.clone(), permit).await
    }
}

pub struct SshReadTransport {
    _session: client::Handle<KnownHostsClient>,
    sftp: SftpSession,
    operation_timeout: Duration,
    broken: AtomicBool,
    _permit: OwnedSemaphorePermit,
}

impl fmt::Debug for SshReadTransport {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SshReadTransport")
            .field("operation_timeout", &self.operation_timeout)
            .field("broken", &self.broken.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl SshReadTransport {
    async fn connect(
        connection: SshConnectionConfig,
        secrets: Arc<dyn SecretResolver>,
        permit: OwnedSemaphorePermit,
    ) -> Result<Self, SshTransportError> {
        let connect_timeout = Duration::from_millis(connection.connect_timeout_millis);
        let operation_timeout = Duration::from_millis(connection.operation_timeout_millis);
        let using_proxy = connection.proxy.is_some();
        let ssh_config = client::Config {
            inactivity_timeout: Some(operation_timeout),
            keepalive_interval: connection.keepalive_seconds.map(Duration::from_secs),
            keepalive_max: 2,
            ..Default::default()
        };
        let handler = KnownHostsClient {
            host: connection.host.clone(),
            port: connection.port,
            known_hosts: connection.host_key.known_hosts_file.clone(),
        };

        let connector = SshStreamConnector;
        let mut session = timeout(connect_timeout, async {
            let stream = connector.connect(&connection).await?;
            client::connect_stream(Arc::new(ssh_config), stream, handler)
                .await
                .map_err(|error| map_connect_error(error, using_proxy))
        })
        .await
        .map_err(|_| {
            if using_proxy {
                SshTransportError::ProxyCommandTimeout
            } else {
                SshTransportError::ConnectTimeout
            }
        })??;

        authenticate(
            &mut session,
            &connection,
            secrets.as_ref(),
            operation_timeout,
        )
        .await?;

        let channel = timeout(operation_timeout, session.channel_open_session())
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::SshProtocol)?;
        timeout(operation_timeout, channel.request_subsystem(true, "sftp"))
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::SshProtocol)?;
        let sftp = timeout(operation_timeout, SftpSession::new(channel.into_stream()))
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::SftpProtocol)?;
        sftp.set_timeout(sftp_timeout_seconds(operation_timeout));

        Ok(Self {
            _session: session,
            sftp,
            operation_timeout,
            broken: AtomicBool::new(false),
            _permit: permit,
        })
    }

    pub async fn stat(&self, path: &str) -> Result<RemoteFileMetadata, SshTransportError> {
        self.ensure_healthy()?;
        validate_remote_path(path)?;
        let result = timeout(self.operation_timeout, self.sftp.metadata(path.to_owned())).await;
        let metadata = self.finish_operation(result, SshTransportError::SftpProtocol)?;
        Ok(metadata.into())
    }

    pub async fn lstat(&self, path: &str) -> Result<RemoteFileMetadata, SshTransportError> {
        self.ensure_healthy()?;
        validate_remote_path(path)?;
        let result = timeout(
            self.operation_timeout,
            self.sftp.symlink_metadata(path.to_owned()),
        )
        .await;
        let metadata = self.finish_operation(result, SshTransportError::SftpProtocol)?;
        Ok(metadata.into())
    }

    pub async fn read_dir(&self, path: &str) -> Result<Vec<RemoteDirEntry>, SshTransportError> {
        self.ensure_healthy()?;
        validate_remote_path(path)?;
        let result = timeout(self.operation_timeout, self.sftp.read_dir(path.to_owned())).await;
        let entries = self.finish_operation(result, SshTransportError::SftpProtocol)?;

        Ok(entries
            .filter_map(|entry| {
                let file_name = entry.file_name().to_owned();
                (file_name != "." && file_name != "..").then_some(RemoteDirEntry { file_name })
            })
            .collect())
    }

    pub async fn read_range(
        &self,
        path: &str,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, SshTransportError> {
        self.ensure_healthy()?;
        validate_remote_path(path)?;
        validate_read_range(offset, length)?;

        let result = timeout(self.operation_timeout, self.sftp.open(path.to_owned())).await;
        let mut file = self.finish_operation(result, SshTransportError::SftpProtocol)?;
        let result = timeout(self.operation_timeout, file.seek(SeekFrom::Start(offset))).await;
        self.finish_operation(result, SshTransportError::SftpProtocol)?;

        let mut buffer = vec![0_u8; length];
        let result = timeout(self.operation_timeout, file.read_exact(&mut buffer)).await;
        self.finish_operation(result, SshTransportError::SftpProtocol)?;

        // russh-sftp 2.3 requires AsyncWriteExt::shutdown() to properly close a File
        // handle. A large sync performs many bounded range reads, so await shutdown
        // before opening the next handle instead of relying on Drop alone.
        let result = timeout(self.operation_timeout, file.shutdown()).await;
        self.finish_operation(result, SshTransportError::SftpProtocol)?;
        Ok(buffer)
    }

    pub async fn close(self) -> Result<(), SshTransportError> {
        timeout(self.operation_timeout, self.sftp.close())
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::SftpProtocol)
    }

    fn ensure_healthy(&self) -> Result<(), SshTransportError> {
        if self.broken.load(Ordering::Acquire) {
            Err(SshTransportError::Broken)
        } else {
            Ok(())
        }
    }

    fn finish_operation<T, E>(
        &self,
        result: Result<Result<T, E>, tokio::time::error::Elapsed>,
        protocol_error: SshTransportError,
    ) -> Result<T, SshTransportError> {
        match result {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => {
                self.broken.store(true, Ordering::Release);
                Err(protocol_error)
            }
            Err(_) => {
                self.broken.store(true, Ordering::Release);
                Err(SshTransportError::OperationTimeout)
            }
        }
    }
}

async fn authenticate(
    session: &mut client::Handle<KnownHostsClient>,
    connection: &SshConnectionConfig,
    secrets: &dyn SecretResolver,
    operation_timeout: Duration,
) -> Result<(), SshTransportError> {
    match connection.auth.auth_type {
        SshAuthType::Password => {
            let secret_ref = connection
                .auth
                .secret_ref
                .as_deref()
                .ok_or(SshTransportError::InvalidConfiguration)?;
            let password = secrets
                .resolve(secret_ref)
                .map_err(|_| SshTransportError::SecretUnavailable)?;
            let result = timeout(
                operation_timeout,
                session.authenticate_password(&connection.username, password.into_exposed()),
            )
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::AuthenticationFailed)?;
            if !result.success() {
                return Err(SshTransportError::AuthenticationFailed);
            }
        }
        SshAuthType::PrivateKey => {
            let key_file = connection
                .auth
                .key_file
                .clone()
                .ok_or(SshTransportError::InvalidConfiguration)?;
            let passphrase = connection
                .auth
                .passphrase_secret_ref
                .as_deref()
                .map(|secret_ref| {
                    secrets
                        .resolve(secret_ref)
                        .map(|secret| secret.into_exposed())
                        .map_err(|_| SshTransportError::SecretUnavailable)
                })
                .transpose()?;
            let key = task::spawn_blocking(move || {
                keys::load_secret_key(key_file, passphrase.as_deref())
            })
            .await
            .map_err(|_| SshTransportError::KeyLoadFailed)?
            .map_err(|_| SshTransportError::KeyLoadFailed)?;
            let hash = timeout(operation_timeout, session.best_supported_rsa_hash())
                .await
                .map_err(|_| SshTransportError::OperationTimeout)?
                .map_err(|_| SshTransportError::AuthenticationFailed)?
                .flatten();
            let key = PrivateKeyWithHashAlg::new(Arc::new(key), hash);
            let result = timeout(
                operation_timeout,
                session.authenticate_publickey(&connection.username, key),
            )
            .await
            .map_err(|_| SshTransportError::OperationTimeout)?
            .map_err(|_| SshTransportError::AuthenticationFailed)?;
            if !result.success() {
                return Err(SshTransportError::AuthenticationFailed);
            }
        }
    }
    Ok(())
}

#[derive(Clone)]
struct KnownHostsClient {
    host: String,
    port: u16,
    known_hosts: std::path::PathBuf,
}

impl client::Handler for KnownHostsClient {
    type Error = SshHandlerError;

    async fn check_server_key(
        &mut self,
        server_public_key: &keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        let host = self.host.clone();
        let port = self.port;
        let known_hosts = self.known_hosts.clone();
        let server_public_key = server_public_key.clone();
        let matched = task::spawn_blocking(move || {
            keys::known_hosts::check_known_hosts_path(&host, port, &server_public_key, &known_hosts)
        })
        .await
        .map_err(|_| SshHandlerError::HostKeyVerificationFailed)?
        .map_err(|_| SshHandlerError::HostKeyVerificationFailed)?;
        if matched {
            Ok(true)
        } else {
            Err(SshHandlerError::HostKeyVerificationFailed)
        }
    }
}

#[derive(Debug, Error)]
enum SshHandlerError {
    #[error("ssh protocol error")]
    Ssh(#[from] russh::Error),
    #[error("host key verification failed")]
    HostKeyVerificationFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SshTransportError {
    #[error("SSH connection configuration is invalid")]
    InvalidConfiguration,
    #[error("unknown SSH connection")]
    UnknownConnection,
    #[error("SSH connection manager is closed")]
    ConnectionManagerClosed,
    #[error("SSH connection concurrency limit wait timed out")]
    ConnectionLimit,
    #[error("SSH connection timed out")]
    ConnectTimeout,
    #[error("SSH connection failed")]
    ConnectFailed,
    #[error("ProxyCommand program was not found")]
    ProxyCommandNotFound,
    #[error("ProxyCommand program is not executable")]
    ProxyCommandPermissionDenied,
    #[error("ProxyCommand could not be started")]
    ProxyCommandStartFailed,
    #[error("ProxyCommand SSH byte stream failed")]
    ProxyCommandStreamFailed,
    #[error("ProxyCommand SSH connection timed out")]
    ProxyCommandTimeout,
    #[error("SSH host key verification failed")]
    HostKeyVerificationFailed,
    #[error("SSH authentication failed")]
    AuthenticationFailed,
    #[error("SSH secret is unavailable")]
    SecretUnavailable,
    #[error("SSH private key could not be loaded")]
    KeyLoadFailed,
    #[error("SSH/SFTP operation timed out")]
    OperationTimeout,
    #[error("SSH protocol operation failed")]
    SshProtocol,
    #[error("SFTP protocol operation failed")]
    SftpProtocol,
    #[error("SSH/SFTP reader is broken")]
    Broken,
    #[error("remote path is invalid")]
    InvalidRemotePath,
    #[error("remote read range is invalid or exceeds the hard limit")]
    InvalidReadRange,
}

fn map_proxy_connect_error(error: ProxyCommandConnectError) -> SshTransportError {
    match error {
        ProxyCommandConnectError::NotConfigured | ProxyCommandConnectError::InvalidArgument => {
            SshTransportError::InvalidConfiguration
        }
        ProxyCommandConnectError::ProgramNotFound => SshTransportError::ProxyCommandNotFound,
        ProxyCommandConnectError::PermissionDenied => {
            SshTransportError::ProxyCommandPermissionDenied
        }
        ProxyCommandConnectError::SpawnFailed | ProxyCommandConnectError::PipeUnavailable => {
            SshTransportError::ProxyCommandStartFailed
        }
    }
}

fn map_connect_error(error: SshHandlerError, using_proxy: bool) -> SshTransportError {
    match error {
        SshHandlerError::HostKeyVerificationFailed => SshTransportError::HostKeyVerificationFailed,
        SshHandlerError::Ssh(_) if using_proxy => SshTransportError::ProxyCommandStreamFailed,
        SshHandlerError::Ssh(_) => SshTransportError::ConnectFailed,
    }
}

fn validate_remote_path(path: &str) -> Result<(), SshTransportError> {
    if path.is_empty() || path.len() > 4096 || path.chars().any(char::is_control) {
        return Err(SshTransportError::InvalidRemotePath);
    }
    Ok(())
}

fn validate_read_range(offset: u64, length: usize) -> Result<(), SshTransportError> {
    if length == 0 || length > MAX_READ_RANGE_BYTES {
        return Err(SshTransportError::InvalidReadRange);
    }
    let length = u64::try_from(length).map_err(|_| SshTransportError::InvalidReadRange)?;
    offset
        .checked_add(length)
        .ok_or(SshTransportError::InvalidReadRange)?;
    Ok(())
}

fn classify_file_type(permissions: Option<u32>) -> RemoteFileType {
    let Some(permissions) = permissions else {
        return RemoteFileType::Unknown;
    };
    match permissions & S_IFMT {
        S_IFREG => RemoteFileType::Regular,
        S_IFDIR => RemoteFileType::Directory,
        S_IFLNK => RemoteFileType::Symlink,
        _ => RemoteFileType::Other,
    }
}

fn sftp_timeout_seconds(timeout: Duration) -> u64 {
    let millis = timeout.as_millis();
    let seconds = millis.saturating_add(999) / 1000;
    u64::try_from(seconds).unwrap_or(u64::MAX).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_remote_file_types_from_posix_mode() {
        assert_eq!(classify_file_type(Some(0o100444)), RemoteFileType::Regular);
        assert_eq!(
            classify_file_type(Some(0o040755)),
            RemoteFileType::Directory
        );
        assert_eq!(classify_file_type(Some(0o120777)), RemoteFileType::Symlink);
        assert_eq!(classify_file_type(None), RemoteFileType::Unknown);
    }

    #[test]
    fn enforces_bounded_exact_range_reads() {
        assert_eq!(validate_read_range(0, 1), Ok(()));
        assert_eq!(validate_read_range(0, MAX_READ_RANGE_BYTES), Ok(()));
        assert_eq!(
            validate_read_range(0, MAX_READ_RANGE_BYTES + 1),
            Err(SshTransportError::InvalidReadRange)
        );
        assert_eq!(
            validate_read_range(u64::MAX, 1),
            Err(SshTransportError::InvalidReadRange)
        );
    }

    #[test]
    fn rejects_control_characters_in_remote_paths() {
        assert_eq!(validate_remote_path("/var/log/app.log"), Ok(()));
        assert_eq!(
            validate_remote_path("/var/log/app.log\nother"),
            Err(SshTransportError::InvalidRemotePath)
        );
    }

    #[test]
    fn sftp_timeout_rounds_up_to_seconds() {
        assert_eq!(sftp_timeout_seconds(Duration::from_millis(100)), 1);
        assert_eq!(sftp_timeout_seconds(Duration::from_millis(1_001)), 2);
    }

    #[test]
    fn maps_proxy_startup_errors_to_stable_transport_categories() {
        assert_eq!(
            map_proxy_connect_error(ProxyCommandConnectError::ProgramNotFound),
            SshTransportError::ProxyCommandNotFound
        );
        assert_eq!(
            map_proxy_connect_error(ProxyCommandConnectError::PermissionDenied),
            SshTransportError::ProxyCommandPermissionDenied
        );
        assert_eq!(
            map_proxy_connect_error(ProxyCommandConnectError::SpawnFailed),
            SshTransportError::ProxyCommandStartFailed
        );
        assert_eq!(
            map_proxy_connect_error(ProxyCommandConnectError::InvalidArgument),
            SshTransportError::InvalidConfiguration
        );
    }
}
