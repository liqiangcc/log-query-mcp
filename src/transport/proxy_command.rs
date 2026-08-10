use std::{
    io,
    pin::Pin,
    process::Stdio,
    task::{Context, Poll},
};

use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    process::{Child, ChildStdin, ChildStdout, Command},
};

use crate::SshConnectionConfig;

pub(crate) struct ProxyCommandStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
    _child: Child,
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

pub(crate) fn connect_proxy_command(
    connection: &SshConnectionConfig,
) -> io::Result<ProxyCommandStream> {
    let proxy = connection.proxy.as_ref().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "proxy command is not configured")
    })?;

    let mut command = Command::new(&proxy.program);
    for argument in &proxy.args {
        command.arg(expand_argument(argument, &connection.host, connection.port)?);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        // M7-3 will replace this with a bounded/redacted stderr collector.
        // Discarding stderr here prevents a verbose helper from blocking the raw SSH stream.
        .stderr(Stdio::null())
        .kill_on_drop(true);

    let mut child = command.spawn()?;
    let stdin = child.stdin.take().ok_or_else(|| {
        io::Error::new(io::ErrorKind::BrokenPipe, "proxy command stdin is unavailable")
    })?;
    let stdout = child.stdout.take().ok_or_else(|| {
        io::Error::new(io::ErrorKind::BrokenPipe, "proxy command stdout is unavailable")
    })?;

    Ok(ProxyCommandStream {
        stdin,
        stdout,
        _child: child,
    })
}

fn expand_argument(argument: &str, host: &str, port: u16) -> io::Result<String> {
    match argument {
        "{host}" => Ok(host.to_owned()),
        "{port}" => Ok(port.to_string()),
        value if value.contains('{') || value.contains('}') => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsupported proxy command placeholder",
        )),
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
        assert!(expand_argument("{username}", "10.20.30.40", 2222).is_err());
        assert!(expand_argument("tcp://{host}:{port}", "10.20.30.40", 2222).is_err());
    }
}
