//! Daemon server — long-running process that holds the browser instance.
//!
//! The daemon listens on `127.0.0.1:<port>` and accepts newline-delimited JSON
//! [`Request`](crate::protocol::Request) messages, executing them against the
//! managed browser and returning [`Response`](crate::protocol::Response)
//! messages.
//!
//! # Lifecycle
//!
//! 1. CLI checks for a running daemon via the port file.
//! 2. If absent, CLI spawns `brother daemon` as a background process.
//! 3. The daemon writes its TCP port to `~/.brother/daemon.port`.
//! 4. CLI connects, sends commands, receives responses.
//! 5. Daemon auto-shuts down after an idle timeout (default 5 min).

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::config::BrowserConfig;
use crate::error::Error;
use crate::page::Page;
use crate::protocol::{
    Request, Response, ResponseData, WaitCondition, WaitStrategy,
};

/// Default idle timeout before the daemon shuts itself down.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Managed browser state shared across connections.
struct DaemonState {
    /// The browser instance (lazily launched).
    browser: Option<crate::Browser>,
    /// The active page.
    page: Option<Page>,
    /// Timestamp of the last command (for idle timeout).
    last_activity: tokio::time::Instant,
}

/// Run the daemon server.
///
/// Binds to a random port on `127.0.0.1`, writes the port to the port file,
/// and serves commands until idle timeout or explicit close.
///
/// # Errors
///
/// Returns an error if binding or port-file writing fails.
pub async fn run(idle_timeout: Option<Duration>) -> crate::Result<()> {
    let timeout = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Browser(format!("failed to bind TCP: {e}")))?;

    let addr = listener
        .local_addr()
        .map_err(|e| Error::Browser(format!("failed to get local addr: {e}")))?;

    // Write port file
    write_port_file(addr.port()).await?;
    write_pid_file().await?;

    tracing::info!(port = addr.port(), "daemon listening");

    let state = Arc::new(Mutex::new(DaemonState {
        browser: None,
        page: None,
        last_activity: tokio::time::Instant::now(),
    }));

    // Idle timeout watcher
    let state_for_idle = Arc::clone(&state);
    let mut idle_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            let last = state_for_idle.lock().await.last_activity;
            if last.elapsed() >= timeout {
                tracing::info!("idle timeout reached, shutting down");
                break;
            }
        }
    });

    // Accept connections until idle timeout or close signal
    loop {
        tokio::select! {
            accept_result = listener.accept() => {
                match accept_result {
                    Ok((stream, peer)) => {
                        tracing::debug!(?peer, "client connected");
                        let state = Arc::clone(&state);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, state).await {
                                tracing::error!(%e, "connection error");
                            }
                        });
                    }
                    Err(e) => {
                        tracing::error!(%e, "accept error");
                    }
                }
            }
            _ = &mut idle_handle => {
                break;
            }
        }
    }

    // Cleanup
    cleanup_files().await;

    // Close browser if still running
    let mut guard = state.lock().await;
    if let Some(browser) = guard.browser.take() {
        let _ = browser.close().await;
    }

    tracing::info!("daemon stopped");
    Ok(())
}

/// Handle a single TCP connection (may carry multiple requests).
async fn handle_connection(
    stream: tokio::net::TcpStream,
    state: Arc<Mutex<DaemonState>>,
) -> crate::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut lines = BufReader::new(reader).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim().to_owned();
        if line.is_empty() {
            continue;
        }

        // Update activity timestamp
        state.lock().await.last_activity = tokio::time::Instant::now();

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let is_close = matches!(req, Request::Close);
                let resp = dispatch(req, &state).await;
                if is_close {
                    // Send response then break
                    let json = serde_json::to_string(&resp).unwrap_or_default();
                    let _ = writer.write_all(json.as_bytes()).await;
                    let _ = writer.write_all(b"\n").await;
                    let _ = writer.flush().await;
                    // Signal shutdown by cleaning up files
                    cleanup_files().await;
                    std::process::exit(0);
                }
                resp
            }
            Err(e) => Response::error(format!("invalid request: {e}")),
        };

        let json = serde_json::to_string(&response).unwrap_or_default();
        if writer.write_all(json.as_bytes()).await.is_err() {
            break;
        }
        if writer.write_all(b"\n").await.is_err() {
            break;
        }
        let _ = writer.flush().await;
    }

    Ok(())
}

/// Dispatch a single request to the appropriate handler.
async fn dispatch(req: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    match req {
        Request::Launch { headless, args } => cmd_launch(state, headless, args).await,
        Request::Navigate { url, wait } => cmd_navigate(state, &url, wait).await,
        Request::Snapshot { options } => cmd_snapshot(state, options).await,
        Request::Click { target } => cmd_click(state, &target).await,
        Request::Fill { target, value } => cmd_fill(state, &target, &value).await,
        Request::Type { target, text } => cmd_type(state, target.as_deref(), &text).await,
        Request::Screenshot { full_page: _ } => cmd_screenshot(state).await,
        Request::Eval { expression } => cmd_eval(state, &expression).await,
        Request::Text { selector } => cmd_text(state, selector.as_deref()).await,
        Request::GetUrl => cmd_get_url(state).await,
        Request::GetTitle => cmd_get_title(state).await,
        Request::Back => cmd_back(state).await,
        Request::Forward => cmd_forward(state).await,
        Request::Reload => cmd_reload(state).await,
        Request::Wait { condition } => cmd_wait(state, condition).await,
        Request::Hover { target } => cmd_hover(state, &target).await,
        Request::Focus { target } => cmd_focus(state, &target).await,
        Request::Status => cmd_status(state).await,
        Request::Close => {
            // Handled in handle_connection, but just in case:
            Response::ok()
        }
    }
}

// ---------------------------------------------------------------------------
// Command handlers
// ---------------------------------------------------------------------------

/// Ensure a browser is launched and return a page.
async fn ensure_browser(state: &Arc<Mutex<DaemonState>>) -> Result<(), Response> {
    let mut guard = state.lock().await;
    if guard.browser.is_none() {
        match launch_browser(BrowserConfig::default()).await {
            Ok((browser, page)) => {
                guard.browser = Some(browser);
                guard.page = Some(page);
            }
            Err(e) => return Err(Response::error(format!("failed to launch browser: {e}"))),
        }
    }
    Ok(())
}

/// Launch a browser and open a blank page, spawning the handler task.
async fn launch_browser(
    config: BrowserConfig,
) -> crate::Result<(crate::Browser, Page)> {
    let (browser, mut handler) = crate::Browser::launch(config).await?;
    // Spawn the CDP handler as a background task
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_blank_page().await?;
    Ok((browser, page))
}

/// Get a reference to the active page, or return an error response.
async fn get_page(state: &Arc<Mutex<DaemonState>>) -> Result<Page, Response> {
    ensure_browser(state).await?;
    let guard = state.lock().await;
    guard
        .page
        .clone()
        .ok_or_else(|| Response::error("no active page"))
}

async fn cmd_launch(
    state: &Arc<Mutex<DaemonState>>,
    _headless: Option<bool>,
    _args: Vec<String>,
) -> Response {
    match ensure_browser(state).await {
        Ok(()) => Response::ok(),
        Err(resp) => resp,
    }
}

async fn cmd_navigate(
    state: &Arc<Mutex<DaemonState>>,
    url: &str,
    wait: WaitStrategy,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    if let Err(e) = page.goto(url).await {
        return Response::error(format!("navigation failed: {e}"));
    }

    // Apply wait strategy
    match wait {
        WaitStrategy::Load | WaitStrategy::DomContentLoaded => {
            // Already handled by the time goto returns
        }
        WaitStrategy::NetworkIdle => {
            if let Err(e) = page.wait_for_navigation().await {
                tracing::warn!(%e, "network idle wait failed");
            }
        }
    }

    // Gather result data
    let result_url = page.url().await.unwrap_or_default();
    let title = page.title().await.unwrap_or_default();

    Response::ok_data(ResponseData::Navigate {
        url: result_url,
        title,
    })
}

async fn cmd_snapshot(
    state: &Arc<Mutex<DaemonState>>,
    options: crate::snapshot::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match page.snapshot_with(options).await {
        Ok(snap) => {
            let refs_json = serde_json::to_value(snap.refs()).unwrap_or_default();
            Response::ok_data(ResponseData::Snapshot {
                tree: snap.tree().to_owned(),
                refs: refs_json,
            })
        }
        Err(e) => Response::error(format!("snapshot failed: {e}")),
    }
}

async fn cmd_click(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let result = if is_ref(target) {
        page.click_ref(target).await
    } else {
        page.click_selector(target).await
    };

    match result {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("click failed: {e}")),
    }
}

async fn cmd_fill(state: &Arc<Mutex<DaemonState>>, target: &str, value: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let result = if is_ref(target) {
        page.fill_ref(target, value).await
    } else {
        page.fill_selector(target, value).await
    };

    match result {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("fill failed: {e}")),
    }
}

async fn cmd_type(
    state: &Arc<Mutex<DaemonState>>,
    target: Option<&str>,
    text: &str,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    if let Some(t) = target {
        if is_ref(t) {
            if let Err(e) = page.type_ref(t, text).await {
                return Response::error(format!("type failed: {e}"));
            }
            return Response::ok();
        }
        // Focus by selector, then type
        if let Err(e) = page.click_selector(t).await {
            return Response::error(format!("focus target failed: {e}"));
        }
    }

    match page.type_text(text).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("type failed: {e}")),
    }
}

async fn cmd_screenshot(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match page.screenshot_png().await {
        Ok(bytes) => {
            let b64 = base64_encode(&bytes);
            Response::ok_data(ResponseData::Screenshot { data: b64 })
        }
        Err(e) => Response::error(format!("screenshot failed: {e}")),
    }
}

async fn cmd_eval(state: &Arc<Mutex<DaemonState>>, expression: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match page.eval(expression).await {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(format!("eval failed: {e}")),
    }
}

async fn cmd_text(state: &Arc<Mutex<DaemonState>>, selector: Option<&str>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let expression = selector.map_or_else(
        || "document.body.innerText || ''".to_owned(),
        |sel| format!("(document.querySelector('{sel}') || document.body).innerText || ''"),
    );

    match page.eval(&expression).await {
        Ok(val) => {
            let text = val.as_str().unwrap_or("").to_owned();
            Response::ok_data(ResponseData::Text { content: text })
        }
        Err(e) => Response::error(format!("text extraction failed: {e}")),
    }
}

async fn cmd_get_url(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match page.url().await {
        Ok(url) => Response::ok_data(ResponseData::Url { url }),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_get_title(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    match page.title().await {
        Ok(title) => Response::ok_data(ResponseData::Title { title }),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_back(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match page.go_back().await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_forward(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match page.go_forward().await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_reload(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match page.reload().await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_wait(state: &Arc<Mutex<DaemonState>>, condition: WaitCondition) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    let result = match condition {
        WaitCondition::Selector {
            selector,
            timeout_ms,
        } => page.wait_for_selector(&selector, Duration::from_millis(timeout_ms)).await,
        WaitCondition::Text {
            text,
            timeout_ms,
        } => page.wait_for_text(&text, Duration::from_millis(timeout_ms)).await,
        WaitCondition::Url {
            pattern,
            timeout_ms,
        } => page.wait_for_url(&pattern, Duration::from_millis(timeout_ms)).await,
        WaitCondition::Function {
            expression,
            timeout_ms,
        } => page.wait_for_function(&expression, Duration::from_millis(timeout_ms)).await,
        WaitCondition::LoadState { state, timeout_ms } => {
            match state {
                WaitStrategy::NetworkIdle => {
                    page.wait_for_network_idle(Duration::from_millis(timeout_ms)).await
                }
                _ => page.wait_for_navigation().await,
            }
        }
        WaitCondition::Duration { ms } => {
            page.wait(Duration::from_millis(ms)).await;
            Ok(())
        }
    };

    match result {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("{e}")),
    }
}

async fn cmd_hover(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match page.hover_ref(target).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("hover failed: {e}")),
    }
}

async fn cmd_focus(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    match page.focus_ref(target).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("focus failed: {e}")),
    }
}

async fn cmd_status(state: &Arc<Mutex<DaemonState>>) -> Response {
    let guard = state.lock().await;
    let browser_running = guard.browser.is_some();
    let page_url = if let Some(ref page) = guard.page {
        page.url().await.ok()
    } else {
        None
    };
    Response::ok_data(ResponseData::Status {
        browser_running,
        page_url,
    })
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Check if a target string is a ref (`@e1`, `e1`, `ref=e1`).
fn is_ref(target: &str) -> bool {
    target.starts_with('@')
        || target.starts_with("ref=")
        || (target.starts_with('e') && target[1..].chars().all(|c| c.is_ascii_digit()))
}


/// Simple base64 encoding without extra crate.
fn base64_encode(data: &[u8]) -> String {
    use std::fmt::Write;
    const CHARS: &[u8; 64] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut result = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };

        let i0 = (b0 >> 2) as usize;
        let i1 = (((b0 & 0x03) << 4) | (b1 >> 4)) as usize;
        let i2 = (((b1 & 0x0F) << 2) | (b2 >> 6)) as usize;
        let i3 = (b2 & 0x3F) as usize;

        let _ = result.write_char(CHARS[i0] as char);
        let _ = result.write_char(CHARS[i1] as char);
        if chunk.len() > 1 {
            let _ = result.write_char(CHARS[i2] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            let _ = result.write_char(CHARS[i3] as char);
        } else {
            result.push('=');
        }
    }
    result
}

/// Write the port number to the runtime directory.
async fn write_port_file(port: u16) -> crate::Result<()> {
    let path = crate::protocol::port_file_path()
        .ok_or_else(|| Error::Browser("cannot determine data directory".into()))?;

    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| Error::Browser(format!("cannot create runtime dir: {e}")))?;
    }

    tokio::fs::write(&path, port.to_string())
        .await
        .map_err(|e| Error::Browser(format!("cannot write port file: {e}")))?;

    tracing::debug!(?path, port, "port file written");
    Ok(())
}

/// Write the daemon PID to the runtime directory.
async fn write_pid_file() -> crate::Result<()> {
    let path = crate::protocol::pid_file_path()
        .ok_or_else(|| Error::Browser("cannot determine data directory".into()))?;

    tokio::fs::write(&path, std::process::id().to_string())
        .await
        .map_err(|e| Error::Browser(format!("cannot write pid file: {e}")))?;

    tracing::debug!(?path, "pid file written");
    Ok(())
}

/// Clean up port and PID files.
async fn cleanup_files() {
    if let Some(path) = crate::protocol::port_file_path() {
        let _ = tokio::fs::remove_file(&path).await;
    }
    if let Some(path) = crate::protocol::pid_file_path() {
        let _ = tokio::fs::remove_file(&path).await;
    }
}
