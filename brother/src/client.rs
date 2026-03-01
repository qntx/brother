//! Daemon client — connects to a running daemon and sends commands.
//!
//! The client handles:
//! 1. Discovering the daemon port from the port file.
//! 2. Auto-starting the daemon if it's not running.
//! 3. Sending [`Request`] messages and receiving [`Response`] messages.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

use crate::error::Error;
use crate::protocol::{self, Request, Response};

/// A client connection to the brother daemon.
#[derive(Debug)]
pub struct DaemonClient {
    stream: BufReader<TcpStream>,
}

impl DaemonClient {
    /// Connect to a running daemon, or start one if needed.
    ///
    /// # Errors
    ///
    /// Returns an error if the daemon cannot be reached or started.
    pub async fn connect() -> crate::Result<Self> {
        // Try connecting to existing daemon first
        if let Some(stream) = try_connect_existing().await {
            return Ok(Self {
                stream: BufReader::new(stream),
            });
        }

        // No daemon running — start one
        start_daemon()?;

        // Wait for daemon to be ready (poll connection)
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(stream) = try_connect_existing().await {
                return Ok(Self {
                    stream: BufReader::new(stream),
                });
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Browser("timeout waiting for daemon to start".into()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Connect to a daemon on a specific port (for testing).
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect_to(port: u16) -> crate::Result<Self> {
        let stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .map_err(|e| Error::Browser(format!("cannot connect to daemon: {e}")))?;
        Ok(Self {
            stream: BufReader::new(stream),
        })
    }

    /// Send a request and wait for the response.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization, I/O, or deserialization fails.
    pub async fn send(&mut self, request: &Request) -> crate::Result<Response> {
        let mut json = serde_json::to_string(request)?;
        json.push('\n');

        self.stream
            .get_mut()
            .write_all(json.as_bytes())
            .await
            .map_err(|e| Error::Browser(format!("send failed: {e}")))?;

        self.stream
            .get_mut()
            .flush()
            .await
            .map_err(|e| Error::Browser(format!("flush failed: {e}")))?;

        let mut line = String::new();
        self.stream
            .read_line(&mut line)
            .await
            .map_err(|e| Error::Browser(format!("read failed: {e}")))?;

        if line.is_empty() {
            return Err(Error::Browser("daemon closed connection".into()));
        }

        serde_json::from_str(&line)
            .map_err(|e| Error::Browser(format!("invalid response from daemon: {e}")))
    }
}

/// Try to connect to an existing daemon by reading the port file.
async fn try_connect_existing() -> Option<TcpStream> {
    let port_file = protocol::port_file_path()?;
    let content = tokio::fs::read_to_string(&port_file).await.ok()?;
    let port: u16 = content.trim().parse().ok()?;
    TcpStream::connect(format!("127.0.0.1:{port}")).await.ok()
}

/// Start a daemon as a background process.
fn start_daemon() -> crate::Result<()> {
    let exe =
        std::env::current_exe().map_err(|e| Error::Browser(format!("cannot find self: {e}")))?;

    // Spawn the daemon as a detached process.
    // The CLI binary should support a hidden "daemon" subcommand.
    #[cfg(windows)]
    {
        use std::process::Stdio;
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("daemon");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        // CREATE_NO_WINDOW on Windows
        cmd.creation_flags(0x0800_0000);
        cmd.spawn()
            .map_err(|e| Error::Browser(format!("cannot spawn daemon: {e}")))?;
    }

    #[cfg(not(windows))]
    {
        use std::process::Stdio;
        let mut cmd = tokio::process::Command::new(&exe);
        cmd.arg("daemon");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd.spawn()
            .map_err(|e| Error::Browser(format!("cannot spawn daemon: {e}")))?;
    }

    tracing::info!(?exe, "daemon process spawned");
    Ok(())
}
