//! Daemon server — long-running process that holds the browser instance.
//!
//! Listens on `127.0.0.1:<port>`, accepts newline-delimited JSON
//! [`Request`](crate::protocol::Request) messages, and returns
//! [`Response`](crate::protocol::Response) messages. The browser is lazily
//! launched on first command.

pub mod auth_vault;
mod dispatch;
mod domain_filter;
mod handlers;
pub mod policy;

use std::sync::Arc;
use std::time::Duration;

use brother::{Browser, BrowserConfig, Error, Page};
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::protocol::{Request, Response};

/// Default idle timeout before auto-shutdown.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Shared state across connections.
struct DaemonState {
    /// Session name for port/pid file management.
    session: String,
    browser: Option<Browser>,
    /// All open tabs (pages). Index 0 is the first tab opened.
    pages: Vec<Page>,
    /// Index into `pages` for the currently active tab.
    active_tab: usize,
    /// Currently active frame (None = main frame).
    active_frame_id: Option<String>,
    /// Active network interception patterns.
    routes: Vec<String>,
    /// Captured network requests (from JS interception).
    captured_requests: Vec<serde_json::Value>,
    /// Download directory path.
    download_path: Option<String>,
    /// Pending launch configuration (set by `Launch` request before browser starts).
    launch_config: Option<BrowserConfig>,
    last_activity: tokio::time::Instant,
    /// Allowed domain patterns for navigation security filter.
    allowed_domains: Vec<String>,
    /// Pending color scheme to apply after browser launch.
    pending_color_scheme: Option<String>,
    /// Action policy cache (hot-reloaded from file).
    policy_cache: Option<policy::PolicyCache>,
    /// HAR recording: captured entries while recording is active.
    har_entries: Option<Vec<serde_json::Value>>,
}

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
        policy_cache: policy_file.and_then(|path| match policy::load_policy_file(path) {
            Ok(p) => {
                tracing::info!(path, "loaded action policy");
                Some(policy::PolicyCache::new(path.to_owned(), p))
            }
            Err(e) => {
                tracing::warn!(path, %e, "failed to load policy file");
                None
            }
        }),
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

/// Execute a page method returning `Result<()>` → `Response::ok()` or error.
///
/// With a target: `page_ok!(state, "target", method(args))` — applies AI-friendly rewrite.
/// Without target: `page_ok!(state, method(args))` — raw error.
macro_rules! page_ok {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly($target).to_string()),
        }
    }};
    ($state:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<serde_json::Value>` → `ResponseData::Eval`.
macro_rules! page_eval {
    ($state:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
            Err(e) => Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<String>` → `ResponseData::Text`.
///
/// With a target: `page_text!(state, "target", method(args))` — applies AI-friendly rewrite.
/// Without target: `page_text!(state, method(args))` — raw error.
macro_rules! page_text {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(text) => Response::ok_data(ResponseData::Text { text }),
            Err(e) => Response::error(e.ai_friendly($target).to_string()),
        }
    }};
    ($state:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(text) => Response::ok_data(ResponseData::Text { text }),
            Err(e) => Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<impl ToString>` with a target → `ResponseData::Text`.
macro_rules! page_display {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(val) => Response::ok_data(ResponseData::Text { text: val.to_string() }),
            Err(e) => Response::error(e.ai_friendly($target).to_string()),
        }
    }};
}

// Make macros available to submodules
pub(crate) use page_display;
pub(crate) use page_eval;
pub(crate) use page_ok;
pub(crate) use page_text;

async fn ensure_browser(state: &Arc<Mutex<DaemonState>>) -> Result<(), Response> {
    let mut guard = state.lock().await;
    if guard.browser.is_none() {
        let config = guard.launch_config.take().unwrap_or_default();
        match launch_browser(config).await {
            Ok((browser, page)) => {
                guard.browser = Some(browser);
                guard.pages = vec![page];
                guard.active_tab = 0;
            }
            Err(e) => return Err(Response::error(format!("launch failed: {e}"))),
        }
    }
    Ok(())
}

async fn launch_browser(config: BrowserConfig) -> brother::Result<(Browser, Page)> {
    let (browser, mut handler) = Browser::launch(config).await?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_blank_page().await?;
    Ok((browser, page))
}

/// Get the active page (tab).
async fn get_page(state: &Arc<Mutex<DaemonState>>) -> Result<Page, Response> {
    ensure_browser(state).await?;
    let guard = state.lock().await;
    guard
        .pages
        .get(guard.active_tab)
        .cloned()
        .ok_or_else(|| Response::error("no active page"))
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

async fn cleanup_files(session: &str) {
    if let Some(p) = crate::protocol::port_file_path_for(session) {
        let _ = tokio::fs::remove_file(&p).await;
    }
    if let Some(p) = crate::protocol::pid_file_path_for(session) {
        let _ = tokio::fs::remove_file(&p).await;
    }
}
