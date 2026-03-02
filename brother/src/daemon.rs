//! Daemon server — long-running process that holds the browser instance.
//!
//! Listens on `127.0.0.1:<port>`, accepts newline-delimited JSON
//! [`Request`](crate::protocol::Request) messages, and returns
//! [`Response`](crate::protocol::Response) messages. The browser is lazily
//! launched on first command.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::config::BrowserConfig;
use crate::error::Error;
use crate::page::Page;
use crate::protocol::{Request, Response, ResponseData, WaitCondition, WaitStrategy};

/// Default idle timeout before auto-shutdown.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Shared state across connections.
struct DaemonState {
    browser: Option<crate::Browser>,
    /// All open tabs (pages). Index 0 is the first tab opened.
    pages: Vec<Page>,
    /// Index into `pages` for the currently active tab.
    active_tab: usize,
    last_activity: tokio::time::Instant,
}

/// Run the daemon server.
///
/// # Errors
///
/// Returns an error if binding or port-file I/O fails.
pub async fn run(idle_timeout: Option<Duration>) -> crate::Result<()> {
    let timeout = idle_timeout.unwrap_or(DEFAULT_IDLE_TIMEOUT);
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| Error::Browser(format!("bind failed: {e}")))?;

    let addr = listener
        .local_addr()
        .map_err(|e| Error::Browser(format!("addr failed: {e}")))?;

    write_port_file(addr.port()).await?;
    write_pid_file().await?;
    tracing::info!(port = addr.port(), "daemon listening");

    let state = Arc::new(Mutex::new(DaemonState {
        browser: None,
        pages: Vec::new(),
        active_tab: 0,
        last_activity: tokio::time::Instant::now(),
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

    cleanup_files().await;
    let browser = state.lock().await.browser.take();
    if let Some(b) = browser {
        let _ = b.close().await;
    }
    tracing::info!("daemon stopped");
    Ok(())
}

// ---------------------------------------------------------------------------
// Connection handler
// ---------------------------------------------------------------------------

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
        state.lock().await.last_activity = tokio::time::Instant::now();

        let response = match serde_json::from_str::<Request>(&line) {
            Ok(req) => {
                let is_close = matches!(req, Request::Close);
                let resp = dispatch(req, &state).await;
                if is_close {
                    send(&mut writer, &resp).await;
                    cleanup_files().await;
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

// ---------------------------------------------------------------------------
// Macros (must be defined before use)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[allow(clippy::cognitive_complexity, clippy::large_stack_frames)] // Pure routing function — splitting would reduce clarity; stack size is dominated by the largest async arm.
async fn dispatch(req: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    match req {
        // Navigation
        Request::Navigate { url, wait } => cmd_navigate(state, &url, wait).await,
        Request::Back => page_ok!(state, go_back()),
        Request::Forward => page_ok!(state, go_forward()),
        Request::Reload => page_ok!(state, reload()),

        // Observation
        Request::Snapshot { options } => cmd_snapshot(state, options).await,
        Request::Screenshot { .. } => cmd_screenshot(state).await,
        Request::Eval { expression } => cmd_eval(state, &expression).await,

        // Interaction — pass target for AI-friendly error rewriting
        Request::Click { target } => page_ok!(state, &target, click(&target)),
        Request::DblClick { target } => page_ok!(state, &target, dblclick(&target)),
        Request::Fill { target, value } => page_ok!(state, &target, fill(&target, &value)),
        Request::Type { target, text } => cmd_type(state, target.as_deref(), &text).await,
        Request::Press { key } => page_ok!(state, key_press(&key)),
        Request::Select { target, value } => {
            page_ok!(state, &target, select_option(&target, &value))
        }
        Request::Check { target } => page_ok!(state, &target, check(&target)),
        Request::Uncheck { target } => page_ok!(state, &target, uncheck(&target)),
        Request::Hover { target } => page_ok!(state, &target, hover(&target)),
        Request::Focus { target } => page_ok!(state, &target, focus(&target)),
        Request::Scroll {
            direction,
            pixels,
            target,
        } => page_ok!(state, scroll(direction, pixels, target.as_deref())),

        // Query — pass target for AI-friendly error rewriting
        Request::GetText { target } => cmd_text(state, target.as_deref()).await,
        Request::GetUrl => page_text!(state, url()),
        Request::GetTitle => page_text!(state, title()),
        Request::GetHtml { target } => page_text!(state, &target, get_html(&target)),
        Request::GetValue { target } => page_text!(state, &target, get_value(&target)),
        Request::GetAttribute { target, attribute } => {
            page_text!(state, &target, get_attribute(&target, &attribute))
        }

        // State checks
        Request::IsVisible { target } => cmd_bool_check(state, &target, "is_visible").await,
        Request::IsEnabled { target } => cmd_bool_check(state, &target, "is_enabled").await,
        Request::IsChecked { target } => cmd_bool_check(state, &target, "is_checked").await,
        Request::Count { selector } => cmd_count(state, &selector).await,

        // Wait
        Request::Wait { condition } => cmd_wait(state, condition).await,

        // Dialog handling
        Request::DialogMessage => cmd_dialog_message(state).await,
        Request::DialogAccept { prompt_text } => {
            page_ok!(state, dialog_accept(prompt_text.as_deref()))
        }
        Request::DialogDismiss => page_ok!(state, dialog_dismiss()),

        // Cookie / Storage
        Request::GetCookies => cmd_get_cookies(state).await,
        Request::SetCookie { cookie } => page_ok!(state, set_cookie(&cookie)),
        Request::ClearCookies => page_ok!(state, clear_cookies()),
        Request::GetStorage { key, session } => {
            page_text!(state, get_storage(&key, session))
        }
        Request::SetStorage {
            key,
            value,
            session,
        } => page_ok!(state, set_storage(&key, &value, session)),
        Request::ClearStorage { session } => page_ok!(state, clear_storage(session)),

        // Tab management
        Request::TabNew { url } => cmd_tab_new(state, url.as_deref()).await,
        Request::TabList => cmd_tab_list(state).await,
        Request::TabSelect { index } => cmd_tab_select(state, index).await,
        Request::TabClose { index } => cmd_tab_close(state, index).await,

        // Debug
        Request::Console => cmd_console(state).await,
        Request::Errors => cmd_errors(state).await,

        // Lifecycle
        Request::Status => cmd_status(state).await,
        Request::Close => Response::ok(),
    }
}

// ---------------------------------------------------------------------------
// Browser lifecycle
// ---------------------------------------------------------------------------

async fn ensure_browser(state: &Arc<Mutex<DaemonState>>) -> Result<(), Response> {
    let mut guard = state.lock().await;
    if guard.browser.is_none() {
        match launch_browser(BrowserConfig::default()).await {
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

async fn launch_browser(config: BrowserConfig) -> crate::Result<(crate::Browser, Page)> {
    let (browser, mut handler) = crate::Browser::launch(config).await?;
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

// ---------------------------------------------------------------------------
// Command handlers (only for non-trivial responses)
// ---------------------------------------------------------------------------

async fn cmd_navigate(state: &Arc<Mutex<DaemonState>>, url: &str, wait: WaitStrategy) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.goto(url).await {
        return Response::error(format!("navigation failed: {e}"));
    }
    if matches!(wait, WaitStrategy::NetworkIdle) {
        let _ = page.wait_for_navigation().await;
    }
    let u = page.url().await.unwrap_or_default();
    let t = page.title().await.unwrap_or_default();
    Response::ok_data(ResponseData::Navigate { url: u, title: t })
}

async fn cmd_snapshot(
    state: &Arc<Mutex<DaemonState>>,
    options: crate::snapshot::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.snapshot_with(options).await {
        Ok(snap) => {
            let refs = serde_json::to_value(snap.refs()).unwrap_or_default();
            Response::ok_data(ResponseData::Snapshot {
                tree: snap.tree().to_owned(),
                refs,
            })
        }
        Err(e) => Response::error(format!("snapshot failed: {e}")),
    }
}

async fn cmd_screenshot(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.screenshot_png().await {
        Ok(bytes) => {
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Response::ok_data(ResponseData::Screenshot { data })
        }
        Err(e) => Response::error(format!("screenshot failed: {e}")),
    }
}

async fn cmd_eval(state: &Arc<Mutex<DaemonState>>, expression: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.eval(expression).await {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(format!("eval failed: {e}")),
    }
}

async fn cmd_type(state: &Arc<Mutex<DaemonState>>, target: Option<&str>, text: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(t) = target {
        if let Err(e) = page.type_into(t, text).await {
            return Response::error(format!("type failed: {e}"));
        }
        return Response::ok();
    }
    match page.type_text(text).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(format!("type failed: {e}")),
    }
}

async fn cmd_text(state: &Arc<Mutex<DaemonState>>, target: Option<&str>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.get_text(target).await {
        Ok(text) => Response::ok_data(ResponseData::Text { text }),
        Err(e) => Response::error(format!("text failed: {e}")),
    }
}

async fn cmd_wait(state: &Arc<Mutex<DaemonState>>, condition: WaitCondition) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let result = match condition {
        WaitCondition::Selector {
            selector,
            timeout_ms,
        } => {
            page.wait_for_selector(&selector, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Text { text, timeout_ms } => {
            page.wait_for_text(&text, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Url {
            pattern,
            timeout_ms,
        } => {
            page.wait_for_url(&pattern, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Function {
            expression,
            timeout_ms,
        } => {
            page.wait_for_function(&expression, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::LoadState {
            state: ws,
            timeout_ms,
        } => match ws {
            WaitStrategy::NetworkIdle => {
                page.wait_for_network_idle(Duration::from_millis(timeout_ms))
                    .await
            }
            _ => page.wait_for_navigation().await,
        },
        WaitCondition::Duration { ms } => {
            page.wait(Duration::from_millis(ms)).await;
            Ok(())
        }
    };
    match result {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}

async fn cmd_dialog_message(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    page.dialog_message().await.map_or_else(
        || {
            Response::ok_data(ResponseData::Text {
                text: "(no dialog)".into(),
            })
        },
        |info| {
            let value = serde_json::to_value(&info).unwrap_or_default();
            Response::ok_data(ResponseData::Eval { value })
        },
    )
}

async fn cmd_get_cookies(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.get_cookies().await {
        Ok(cookies) => Response::ok_data(ResponseData::Eval { value: cookies }),
        Err(e) => Response::error(format!("get cookies failed: {e}")),
    }
}

async fn cmd_bool_check(state: &Arc<Mutex<DaemonState>>, target: &str, method: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let result = match method {
        "is_visible" => page.is_visible(target).await,
        "is_enabled" => page.is_enabled(target).await,
        "is_checked" => page.is_checked(target).await,
        _ => return Response::error(format!("unknown check: {method}")),
    };
    match result {
        Ok(val) => Response::ok_data(ResponseData::Text {
            text: val.to_string(),
        }),
        Err(e) => Response::error(e.ai_friendly(target).to_string()),
    }
}

async fn cmd_count(state: &Arc<Mutex<DaemonState>>, selector: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.count(selector).await {
        Ok(n) => Response::ok_data(ResponseData::Text {
            text: n.to_string(),
        }),
        Err(e) => Response::error(e.ai_friendly(selector).to_string()),
    }
}

async fn cmd_tab_new(state: &Arc<Mutex<DaemonState>>, url: Option<&str>) -> Response {
    ensure_browser(state).await.ok();
    let mut guard = state.lock().await;
    let Some(ref browser) = guard.browser else {
        return Response::error("no browser running");
    };
    let target = url.unwrap_or("about:blank");
    match browser.new_page(target).await {
        Ok(page) => {
            guard.pages.push(page);
            guard.active_tab = guard.pages.len() - 1;
            Response::ok_data(ResponseData::Text {
                text: format!("tab {} opened", guard.active_tab),
            })
        }
        Err(e) => Response::error(format!("tab new failed: {e}")),
    }
}

async fn cmd_tab_list(state: &Arc<Mutex<DaemonState>>) -> Response {
    ensure_browser(state).await.ok();
    let guard = state.lock().await;
    let mut tabs = Vec::new();
    for (i, page) in guard.pages.iter().enumerate() {
        let url = page.url().await.unwrap_or_default();
        tabs.push(serde_json::json!({
            "index": i,
            "url": url,
            "active": i == guard.active_tab,
        }));
    }
    let active = guard.active_tab;
    Response::ok_data(ResponseData::TabList {
        tabs: serde_json::Value::Array(tabs),
        active,
    })
}

async fn cmd_tab_select(state: &Arc<Mutex<DaemonState>>, index: usize) -> Response {
    ensure_browser(state).await.ok();
    let mut guard = state.lock().await;
    if index >= guard.pages.len() {
        return Response::error(format!(
            "tab index {index} out of range (0..{})",
            guard.pages.len()
        ));
    }
    guard.active_tab = index;
    Response::ok_data(ResponseData::Text {
        text: format!("switched to tab {index}"),
    })
}

async fn cmd_tab_close(state: &Arc<Mutex<DaemonState>>, index: Option<usize>) -> Response {
    ensure_browser(state).await.ok();
    let mut guard = state.lock().await;
    let idx = index.unwrap_or(guard.active_tab);
    if idx >= guard.pages.len() {
        return Response::error(format!(
            "tab index {idx} out of range (0..{})",
            guard.pages.len()
        ));
    }
    if guard.pages.len() == 1 {
        return Response::error("cannot close the last tab");
    }
    guard.pages.remove(idx);
    // Adjust active_tab if needed
    if guard.active_tab >= guard.pages.len() {
        guard.active_tab = guard.pages.len() - 1;
    }
    Response::ok_data(ResponseData::Text {
        text: format!("tab {idx} closed, active tab: {}", guard.active_tab),
    })
}

async fn cmd_console(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let logs = page.take_console_logs().await;
    let entries = serde_json::to_value(&logs).unwrap_or_default();
    Response::ok_data(ResponseData::Logs { entries })
}

async fn cmd_errors(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let errors = page.take_js_errors().await;
    let entries = serde_json::to_value(&errors).unwrap_or_default();
    Response::ok_data(ResponseData::Logs { entries })
}

async fn cmd_status(state: &Arc<Mutex<DaemonState>>) -> Response {
    let guard = state.lock().await;
    let browser_running = guard.browser.is_some();
    let page_url: Option<String> = if let Some(page) = guard.pages.get(guard.active_tab) {
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
// File helpers
// ---------------------------------------------------------------------------

async fn write_port_file(port: u16) -> crate::Result<()> {
    let path = crate::protocol::port_file_path()
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

async fn write_pid_file() -> crate::Result<()> {
    let path = crate::protocol::pid_file_path()
        .ok_or_else(|| Error::Browser("cannot determine data dir".into()))?;
    tokio::fs::write(&path, std::process::id().to_string())
        .await
        .map_err(|e| Error::Browser(format!("write pid file: {e}")))?;
    tracing::debug!(?path, "pid file written");
    Ok(())
}

async fn cleanup_files() {
    if let Some(p) = crate::protocol::port_file_path() {
        let _ = tokio::fs::remove_file(&p).await;
    }
    if let Some(p) = crate::protocol::pid_file_path() {
        let _ = tokio::fs::remove_file(&p).await;
    }
}
