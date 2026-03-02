//! Daemon client — connects to a running daemon and sends commands.
//!
//! The client handles:
//! 1. Discovering the daemon port from the port file.
//! 2. Auto-starting the daemon if it's not running.
//! 3. Sending [`Request`] messages and receiving [`Response`] messages.

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;

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
    pub async fn connect() -> anyhow::Result<Self> {
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
                anyhow::bail!("timeout waiting for daemon to start");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Send a request and wait for the response.
    ///
    /// # Errors
    ///
    /// Returns an error if serialization, I/O, or deserialization fails.
    pub async fn send(&mut self, request: &Request) -> anyhow::Result<Response> {
        let mut json = serde_json::to_string(request)?;
        json.push('\n');

        self.stream
            .get_mut()
            .write_all(json.as_bytes())
            .await
            .map_err(|e| anyhow::anyhow!("send failed: {e}"))?;

        self.stream
            .get_mut()
            .flush()
            .await
            .map_err(|e| anyhow::anyhow!("flush failed: {e}"))?;

        let mut line = String::new();
        self.stream
            .read_line(&mut line)
            .await
            .map_err(|e| anyhow::anyhow!("read failed: {e}"))?;

        if line.is_empty() {
            anyhow::bail!("daemon closed connection");
        }

        serde_json::from_str(&line)
            .map_err(|e| anyhow::anyhow!("invalid response from daemon: {e}"))
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
fn start_daemon() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;

    // Spawn the daemon as a detached process.
    // The CLI binary should support a hidden "daemon" subcommand.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        std::process::Command::new(&exe)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // CREATE_NO_WINDOW on Windows
            .creation_flags(0x0800_0000)
            .spawn()?;
    }

    #[cfg(not(windows))]
    {
        use std::process::Stdio;
        std::process::Command::new(&exe)
            .arg("daemon")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
    }

    Ok(())
}
