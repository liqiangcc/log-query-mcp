use std::{
    io,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
};

use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::Handle,
    task::JoinHandle,
};

use crate::SshConnectionConfig;

const MAX_PROXY_STDERR_BYTES: usize = 64 * 1024;
const PROXY_STDERR_READ_BUFFER_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum ProxyCommandConnectError {
    #[error("proxy command is not configured")]
    NotConfigured,
    #[error("proxy command argument is invalid")]
    InvalidArgument,
    #[error("proxy command program was not found")]
    ProgramNotFound,
    #[error("proxy command program is not executable")]
    PermissionDenied,
    #[error("proxy command could not be started")]
    SpawnFailed,
    #[error("proxy command stdio pipe is unavailable")]
    PipeUnavailable,
}

pub(crate) struct ProxyCommandStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    child: Option<Child>,
    stderr_task: Option<JoinHandle<()>>,
}

impl AsyncRead for ProxyCommandStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for ProxyCommandStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

impl Drop for ProxyCommandStream {
    fn drop(&mut self) {
        if let Some(task) = self.stderr_task.take() {
            task.abort();
        }

        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();

        // Reap asynchronously whenever a Tokio runtime is still available. kill_on_drop remains
        // enabled as a final fail-closed guard for runtime-shutdown paths.
        if let Ok(handle) = Handle::try_current() {
            let _reaper = handle.spawn(async move {
                let _ = child.wait().await;
            });
        }
    }
}

pub(crate) fn connect_proxy_command(
    connection: &SshConnectionConfig,
) -> Result<ProxyCommandStream, ProxyCommandConnectError> {
    let proxy = connection
        .proxy
        .as_ref()
        .ok_or(ProxyCommandConnectError::NotConfigured)?;

    let mut command = Command::new(&proxy.program);
    for argument in &proxy.args {
        command.arg(expand_argument(
            argument,
            &connection.host,
            connection.port,
        )?);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = command.spawn().map_err(classify_spawn_error)?;
    let stdin = child
        .stdin
        .take()
        .ok_or(ProxyCommandConnectError::PipeUnavailable)?;
    let stdout = child
        .stdout
        .take()
        .ok_or(ProxyCommandConnectError::PipeUnavailable)?;
    let stderr = child
        .stderr
        .take()
        .ok_or(ProxyCommandConnectError::PipeUnavailable)?;
    let stderr_task = tokio::spawn(async move {
        drain_bounded_stderr(stderr).await;
    });

    Ok(ProxyCommandStream {
        stdin,
        stdout,
        child: Some(child),
        stderr_task: Some(stderr_task),
    })
}

fn classify_spawn_error(error: io::Error) -> ProxyCommandConnectError {
    match error.kind() {
        io::ErrorKind::NotFound => ProxyCommandConnectError::ProgramNotFound,
        io::ErrorKind::PermissionDenied => ProxyCommandConnectError::PermissionDenied,
        _ => ProxyCommandConnectError::SpawnFailed,
    }
}

async fn drain_bounded_stderr(mut stderr: tokio::process::ChildStderr) {
    let mut captured = Vec::with_capacity(MAX_PROXY_STDERR_BYTES);
    let mut buffer = [0_u8; PROXY_STDERR_READ_BUFFER_BYTES];

    loop {
        let read = match stderr.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        if captured.len() < MAX_PROXY_STDERR_BYTES {
            let remaining = MAX_PROXY_STDERR_BYTES - captured.len();
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        // Continue draining after the capture limit so a verbose helper cannot block on a full
        // stderr pipe. Captured bytes are deliberately not logged or returned in M7-3.
    }
}

fn expand_argument(
    argument: &str,
    host: &str,
    port: u16,
) -> Result<String, ProxyCommandConnectError> {
    match argument {
        "{host}" => Ok(host.to_owned()),
        "{port}" => Ok(port.to_string()),
        value if value.contains('{') || value.contains('}') => {
            Err(ProxyCommandConnectError::InvalidArgument)
        }
        value => Ok(value.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_only_frozen_proxy_placeholders() {
        assert_eq!(
            expand_argument("{host}", "10.20.30.40", 2222).expect("host should expand"),
            "10.20.30.40"
        );
        assert_eq!(
            expand_argument("{port}", "10.20.30.40", 2222).expect("port should expand"),
            "2222"
        );
        assert_eq!(
            expand_argument("--stdio", "10.20.30.40", 2222).expect("literal should pass"),
            "--stdio"
        );
        assert_eq!(
            expand_argument("{username}", "10.20.30.40", 2222),
            Err(ProxyCommandConnectError::InvalidArgument)
        );
        assert_eq!(
            expand_argument("tcp://{host}:{port}", "10.20.30.40", 2222),
            Err(ProxyCommandConnectError::InvalidArgument)
        );
    }

    #[test]
    fn classifies_spawn_errors_without_exposing_os_details() {
        assert_eq!(
            classify_spawn_error(io::Error::from(io::ErrorKind::NotFound)),
            ProxyCommandConnectError::ProgramNotFound
        );
        assert_eq!(
            classify_spawn_error(io::Error::from(io::ErrorKind::PermissionDenied)),
            ProxyCommandConnectError::PermissionDenied
        );
        assert_eq!(
            classify_spawn_error(io::Error::from(io::ErrorKind::Other)),
            ProxyCommandConnectError::SpawnFailed
        );
    }
}
