//! TCP server, connection handling, and port/pid file management.

use std::sync::Arc;
use std::time::Duration;

use brother::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::protocol::{Request, Response};

use super::dispatch;
use super::state::DaemonState;

/// Default idle timeout before auto-shutdown.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Run the daemon server for a named session.
///
/// # Errors
///
/// Returns an error if binding or port-file I/O fails.
pub async fn run_session(
    session: &str,
    idle_timeout: Option<Duration>,
    policy_file: Option<&str>,
) -> brother::Result<()> {
    let timeout = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let session_name = session.to_owned();
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Browser(format!("bind failed: {e}")))?;

    let addr = listener
        .local_addr()
        .map_err(|e| Error::Browser(format!("addr failed: {e}")))?;

    write_port_file(&session_name, addr.port()).await?;
    write_pid_file(&session_name).await?;
    tracing::info!(port = addr.port(), session = %session_name, "daemon listening");

    let state = Arc::new(Mutex::new(DaemonState {
        session: session_name.clone(),
        browser: None,
        pages: Vec::new(),
        active_tab: 0,
        active_frame_id: None,
        routes: Vec::new(),
        captured_requests: Vec::new(),
        download_path: None,
        launch_config: None,
        last_activity: tokio::time::Instant::now(),
        allowed_domains: Vec::new(),
        pending_color_scheme: None,
        pending_storage_state: None,
        policy_cache: policy_file.and_then(|path| {
            match crate::policy::load_policy_file(path) {
                Ok(p) => {
                    tracing::info!(path, "loaded action policy");
                    Some(crate::policy::PolicyCache::new(path.to_owned(), p))
                }
                Err(e) => {
                    tracing::warn!(path, %e, "failed to load policy file");
                    None
                }
            }
        }),
        confirmations: crate::policy::ConfirmationQueue::new(),
        har_entries: None,
    }));

    // Idle watcher
    let idle_state = Arc::clone(&state);
    let mut idle_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            if idle_state.lock().await.last_activity.elapsed() >= timeout {
                tracing::info!("idle timeout, shutting down");
                break;
            }
        }
    });

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer)) => {
                        tracing::debug!(?peer, "connected");
                        let st = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, st).await {
                                tracing::error!(%e, "connection error");
                            }
                        });
                    }
                    Err(e) => tracing::error!(%e, "accept error"),
                }
            }
            _ = &mut idle_handle => break,
        }
    }

    cleanup_files(&session_name).await;
    let browser = state.lock().await.browser.take();
    if let Some(b) = browser {
        let _ = b.close().await;
    }
    tracing::info!("daemon stopped");
    Ok(())
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<DaemonState>>,
) -> brother::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }
        state.lock().await.last_activity = tokio::time::Instant::now();

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let is_close = matches!(req, Request::Close);
                let resp = dispatch::dispatch(req, &state).await;
                if is_close {
                    send(&mut writer, &resp).await;
                    let session = state.lock().await.session.clone();
                    cleanup_files(&session).await;
                    std::process::exit(0);
                }
                resp
            }
            Err(e) => Response::error(format!("invalid request: {e}")),
        };

        if !send(&mut writer, &response).await {
            break;
        }
    }
    Ok(())
}

/// Send a JSON response over the wire. Returns `false` on write error.
async fn send(writer: &mut tokio::net::tcp::OwnedWriteHalf, resp: &Response) -> bool {
    let json = serde_json::to_string(resp).unwrap_or_default();
    if writer.write_all(json.as_bytes()).await.is_err() || writer.write_all(b"\n").await.is_err() {
        return false;
    }
    let _ = writer.flush().await;
    true
}

async fn write_port_file(session: &str, port: u16) -> brother::Result<()> {
    let path = crate::protocol::port_file_path_for(session)
        .ok_or_else(|| Error::Browser("cannot determine data dir".into()))?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Browser(format!("mkdir failed: {e}")))?;
    }
    tokio::fs::write(&path, port.to_string())
        .await
        .map_err(|e| Error::Browser(format!("write port file: {e}")))?;
    tracing::debug!(?path, port, "port file written");
    Ok(())
}

async fn write_pid_file(session: &str) -> brother::Result<()> {
    let path = crate::protocol::pid_file_path_for(session)
        .ok_or_else(|| Error::Browser("cannot determine data dir".into()))?;
    tokio::fs::write(&path, std::process::id().to_string())
        .await
        .map_err(|e| Error::Browser(format!("write pid file: {e}")))?;
    tracing::debug!(?path, "pid file written");
    Ok(())
}

pub(crate) async fn cleanup_files(session: &str) {
    if let Some(p) = crate::protocol::port_file_path_for(session) {
        let _ = tokio::fs::remove_file(&p).await;
    }
    if let Some(p) = crate::protocol::pid_file_path_for(session) {
        let _ = tokio::fs::remove_file(&p).await;
    }
}
