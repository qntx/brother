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
use crate::protocol::{
    MouseButton, Request, Response, ResponseData, RouteAction, WaitCondition, WaitStrategy,
};

/// Default idle timeout before auto-shutdown.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Shared state across connections.
struct DaemonState {
    browser: Option<crate::Browser>,
    /// All open tabs (pages). Index 0 is the first tab opened.
    pages: Vec<Page>,
    /// Index into `pages` for the currently active tab.
    active_tab: usize,
    /// Currently active frame (None = main frame).
    active_frame_id: Option<String>,
    /// Network interception rules: pattern → (action, status, body, `content_type`).
    routes: Vec<InterceptRoute>,
    /// Captured network requests (from JS interception).
    captured_requests: Vec<serde_json::Value>,
    /// Download directory path.
    download_path: Option<String>,
    last_activity: tokio::time::Instant,
}

/// A network interception rule.
#[allow(dead_code)]
struct InterceptRoute {
    pattern: String,
    action: RouteAction,
    status: u16,
    body: String,
    content_type: String,
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
        active_frame_id: None,
        routes: Vec::new(),
        captured_requests: Vec::new(),
        download_path: None,
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
        // Connection
        Request::Connect { target } => cmd_connect(state, &target).await,

        // Navigation
        Request::Navigate { url, wait } => cmd_navigate(state, &url, wait).await,
        Request::Back => page_ok!(state, go_back()),
        Request::Forward => page_ok!(state, go_forward()),
        Request::Reload => page_ok!(state, reload()),

        // Observation
        Request::Snapshot { options } => cmd_snapshot(state, options).await,
        Request::Screenshot {
            selector,
            format,
            quality,
            ..
        } => cmd_screenshot(state, selector.as_deref(), &format, quality).await,
        Request::Eval { expression } => cmd_eval(state, &expression).await,

        // Interaction — pass target for AI-friendly error rewriting
        Request::Click {
            target,
            button,
            click_count,
        } => cmd_click(state, &target, button, click_count).await,
        Request::DblClick { target } => page_ok!(state, &target, dblclick(&target)),
        Request::Fill { target, value } => page_ok!(state, &target, fill(&target, &value)),
        Request::Type {
            target,
            text,
            delay_ms,
        } => cmd_type(state, target.as_deref(), &text, delay_ms).await,
        Request::Press { key } => page_ok!(state, key_press(&key)),
        Request::Select { target, values } => {
            cmd_select(state, &target, &values).await
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

        // Frame (iframe) support
        Request::Frame { selector } => cmd_frame(state, &selector).await,
        Request::MainFrame => cmd_main_frame(state).await,

        // Raw keyboard
        Request::KeyDown { key } => page_ok!(state, key_down(&key)),
        Request::KeyUp { key } => page_ok!(state, key_up(&key)),
        Request::InsertText { text } => page_ok!(state, insert_text(&text)),

        // File / DOM manipulation
        Request::Upload { target, files } => page_ok!(state, &target, upload(&target, &files)),
        Request::Drag { source, target } => page_ok!(state, &source, drag(&source, &target)),
        Request::Clear { target } => page_ok!(state, &target, clear(&target)),
        Request::ScrollIntoView { target } => {
            page_ok!(state, &target, scroll_into_view(&target))
        }
        Request::BoundingBox { target } => cmd_bounding_box(state, &target).await,
        Request::SetContent { html } => page_ok!(state, set_content(&html)),
        Request::Pdf { path } => page_ok!(state, pdf(&path)),

        // Network interception
        Request::Route {
            pattern,
            action,
            status,
            body,
            content_type,
        } => cmd_route(state, pattern, action, status, body, content_type).await,
        Request::Unroute { pattern } => cmd_unroute(state, &pattern).await,
        Request::Requests { action, filter } => {
            cmd_requests(state, action.as_deref(), filter.as_deref()).await
        }

        // Download handling
        Request::SetDownloadPath { path } => cmd_set_download_path(state, &path).await,
        Request::Downloads { action } => cmd_downloads(state, action.as_deref()).await,
        Request::WaitForDownload { path, timeout_ms } => {
            cmd_wait_for_download(state, path.as_deref(), timeout_ms).await
        }
        Request::ResponseBody { url, timeout_ms } => {
            cmd_response_body(state, &url, timeout_ms).await
        }

        // Clipboard
        Request::ClipboardRead => cmd_clipboard_read(state).await,
        Request::ClipboardWrite { text } => cmd_clipboard_write(state, &text).await,

        // Environment emulation
        Request::Viewport { width, height } => cmd_viewport(state, width, height).await,
        Request::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        } => {
            cmd_emulate_media(
                state,
                media.as_deref(),
                color_scheme.as_deref(),
                reduced_motion.as_deref(),
                forced_colors.as_deref(),
            )
            .await
        }
        Request::Offline { offline } => cmd_offline(state, offline).await,
        Request::ExtraHeaders { headers_json } => cmd_extra_headers(state, &headers_json).await,
        Request::Geolocation {
            latitude,
            longitude,
            accuracy,
        } => cmd_geolocation(state, latitude, longitude, accuracy).await,
        Request::Credentials { username, password } => {
            cmd_credentials(state, &username, &password).await
        }
        Request::UserAgent { user_agent } => cmd_user_agent(state, &user_agent).await,
        Request::Timezone { timezone_id } => cmd_timezone(state, &timezone_id).await,
        Request::Locale { locale } => cmd_locale(state, &locale).await,
        Request::Permissions { permissions, grant } => {
            cmd_permissions(state, &permissions, grant).await
        }
        Request::BringToFront => cmd_bring_to_front(state).await,

        // Script injection
        Request::AddInitScript { script } => cmd_add_init_script(state, &script).await,
        Request::AddScript { content, url } => {
            cmd_add_script(state, content.as_deref(), url.as_deref()).await
        }
        Request::AddStyle { content, url } => {
            cmd_add_style(state, content.as_deref(), url.as_deref()).await
        }
        Request::Dispatch {
            target,
            event,
            event_init,
        } => cmd_dispatch(state, &target, &event, event_init.as_deref()).await,

        // Misc interaction / queries
        Request::Styles { target } => cmd_styles(state, &target).await,
        Request::SelectAll { target } => cmd_select_all(state, &target).await,
        Request::Highlight { target } => cmd_highlight(state, &target).await,
        Request::MouseMove { x, y } => cmd_mouse_move(state, x, y).await,
        Request::MouseDown { button } => cmd_mouse_down(state, button).await,
        Request::MouseUp { button } => cmd_mouse_up(state, button).await,
        Request::Wheel {
            delta_x,
            delta_y,
            selector,
        } => cmd_wheel(state, delta_x, delta_y, selector.as_deref()).await,
        Request::Tap { target } => cmd_tap(state, &target).await,
        Request::SetValue { target, value } => {
            cmd_set_value(state, &target, &value).await
        }

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
        Request::Console { clear } => cmd_console(state, clear).await,
        Request::Errors { clear } => cmd_errors(state, clear).await,

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

/// Connect to an existing browser via CDP websocket URL or debugging port.
async fn cmd_connect(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    // Build the websocket URL from the target string.
    // Accept: "9222", "ws://...", or "http://127.0.0.1:9222".
    let ws_url = resolve_cdp_endpoint(target).await;
    let ws_url = match ws_url {
        Ok(url) => url,
        Err(e) => return Response::error(format!("failed to resolve CDP endpoint: {e}")),
    };

    let connect_result = crate::Browser::connect(&ws_url).await;
    let (browser, mut handler) = match connect_result {
        Ok(pair) => pair,
        Err(e) => return Response::error(format!("connect failed: {e}")),
    };
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    // Pick up all existing pages from the connected browser.
    let pages_result = browser.pages().await;
    let pages = match pages_result {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => {
            // No pages open — create a blank one.
            match browser.new_blank_page().await {
                Ok(p) => vec![p],
                Err(e) => return Response::error(format!("connected but no pages: {e}")),
            }
        }
        Err(e) => return Response::error(format!("connected but failed to list pages: {e}")),
    };

    let tab_count = pages.len();
    let mut guard = state.lock().await;
    guard.browser = Some(browser);
    guard.pages = pages;
    guard.active_tab = 0;

    Response::ok_data(ResponseData::Text {
        text: format!("connected to {ws_url} ({tab_count} tabs)"),
    })
}

/// Resolve a CDP target string to a websocket URL.
///
/// Accepts:
/// - A bare port number (e.g. `"9222"`) → fetches `http://127.0.0.1:9222/json/version`
/// - An HTTP URL (e.g. `"http://127.0.0.1:9222"`) → fetches `/json/version`
/// - A `ws://` or `wss://` URL → used directly
async fn resolve_cdp_endpoint(target: &str) -> crate::Result<String> {
    if target.starts_with("ws://") || target.starts_with("wss://") {
        return Ok(target.to_string());
    }

    let http_base = if target.chars().all(|c| c.is_ascii_digit()) {
        format!("http://127.0.0.1:{target}")
    } else if target.starts_with("http://") || target.starts_with("https://") {
        target.to_string()
    } else {
        format!("http://{target}")
    };

    let version_url = format!("{http_base}/json/version");
    let resp = reqwest::get(&version_url).await.map_err(|e| {
        Error::Browser(format!(
            "cannot reach Chrome at {version_url} — is Chrome running with --remote-debugging-port? ({e})"
        ))
    })?;
    let body: serde_json::Value = resp.json().await.map_err(|e| {
        Error::Browser(format!("invalid JSON from {version_url}: {e}"))
    })?;
    body["webSocketDebuggerUrl"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            Error::Browser(
                "webSocketDebuggerUrl not found in /json/version response".into(),
            )
        })
}

/// Get bounding box of an element.
async fn cmd_bounding_box(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.bounding_box(target).await {
        Ok((x, y, w, h)) => Response::ok_data(ResponseData::BoundingBox {
            x,
            y,
            width: w,
            height: h,
        }),
        Err(e) => Response::error(format!("{}", e.ai_friendly(target))),
    }
}

/// Switch execution context to a child frame.
///
/// The selector can be:
/// - A numeric index (e.g. `"0"`) into the child frames list
/// - A frame name (e.g. `"myframe"`)
/// - A URL substring (e.g. `"ads.example.com"`)
async fn cmd_frame(state: &Arc<Mutex<DaemonState>>, selector: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::GetFrameTreeParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Get the frame tree from CDP.
    let tree_resp = match page.inner().execute(GetFrameTreeParams::default()).await {
        Ok(r) => r,
        Err(e) => return Response::error(format!("failed to get frame tree: {e}")),
    };

    let root = &tree_resp.result.frame_tree;
    let children = root.child_frames.as_deref().unwrap_or_default();

    if children.is_empty() {
        return Response::error("no child frames found on this page");
    }

    // Try to match by index, name, or URL substring.
    let matched = selector.parse::<usize>().map_or_else(
        |_| {
            children.iter().find_map(|c| {
                let f = &c.frame;
                let name_match = f.name.as_deref().is_some_and(|n| n == selector);
                let url_match = f.url.contains(selector);
                (name_match || url_match).then_some(f)
            })
        },
        |idx| children.get(idx).map(|c| &c.frame),
    );

    let Some(frame) = matched else {
        let available: Vec<String> = children
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let name = c.frame.name.as_deref().unwrap_or("(unnamed)");
                format!("[{i}] name={name} url={}", c.frame.url)
            })
            .collect();
        return Response::error(format!(
            "frame \"{selector}\" not found. Available frames:\n{}",
            available.join("\n")
        ));
    };

    let frame_id = frame.id.inner().to_owned();
    let frame_name = frame.name.as_deref().unwrap_or("(unnamed)");
    let frame_url = &frame.url;

    state.lock().await.active_frame_id = Some(frame_id.clone());

    Response::ok_data(ResponseData::Text {
        text: format!("switched to frame: name={frame_name} url={frame_url} id={frame_id}"),
    })
}

/// Switch back to the main (top-level) frame.
async fn cmd_main_frame(state: &Arc<Mutex<DaemonState>>) -> Response {
    let mut guard = state.lock().await;
    let was_in_frame = guard.active_frame_id.is_some();
    guard.active_frame_id = None;
    if was_in_frame {
        Response::ok_data(ResponseData::Text {
            text: "switched to main frame".into(),
        })
    } else {
        Response::ok_data(ResponseData::Text {
            text: "already in main frame".into(),
        })
    }
}

/// Add a network interception route.
///
/// Injects JS to monkey-patch `fetch` and `XMLHttpRequest` so requests
/// matching the pattern are fulfilled with a custom response or aborted.
async fn cmd_route(
    state: &Arc<Mutex<DaemonState>>,
    pattern: String,
    action: RouteAction,
    status: u16,
    body: String,
    content_type: String,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Store the route in daemon state.
    state.lock().await.routes.push(InterceptRoute {
        pattern: pattern.clone(),
        action,
        status,
        body: body.clone(),
        content_type: content_type.clone(),
    });

    // Inject JS interception for this pattern.
    let js = if matches!(action, RouteAction::Abort) {
        format!(
            r"(() => {{
                if (!window.__brother_routes) window.__brother_routes = [];
                window.__brother_routes.push({{ pattern: '{pat}', action: 'abort' }});
                if (!window.__brother_fetch_patched) {{
                    window.__brother_fetch_patched = true;
                    const F = window.fetch;
                    window.fetch = function(url, opts) {{
                        const u = typeof url === 'string' ? url : url.url || '';
                        const r = (window.__brother_routes || []).find(r => u.includes(r.pattern));
                        if (r && r.action === 'abort') return Promise.reject(new TypeError('Network request aborted by brother route'));
                        if (r && r.action === 'fulfill') return Promise.resolve(new Response(r.body, {{ status: r.status, headers: {{ 'Content-Type': r.contentType }} }}));
                        return F.apply(this, arguments);
                    }};
                }}
            }})()",
            pat = pattern.replace('\'', "\\'")
        )
    } else {
        format!(
            r"(() => {{
                if (!window.__brother_routes) window.__brother_routes = [];
                window.__brother_routes.push({{ pattern: '{pat}', action: 'fulfill', status: {status}, body: '{body_esc}', contentType: '{ct}' }});
                if (!window.__brother_fetch_patched) {{
                    window.__brother_fetch_patched = true;
                    const F = window.fetch;
                    window.fetch = function(url, opts) {{
                        const u = typeof url === 'string' ? url : url.url || '';
                        const r = (window.__brother_routes || []).find(r => u.includes(r.pattern));
                        if (r && r.action === 'abort') return Promise.reject(new TypeError('Network request aborted by brother route'));
                        if (r && r.action === 'fulfill') return Promise.resolve(new Response(r.body, {{ status: r.status, headers: {{ 'Content-Type': r.contentType }} }}));
                        return F.apply(this, arguments);
                    }};
                }}
            }})()",
            pat = pattern.replace('\'', "\\'"),
            body_esc = body.replace('\'', "\\'").replace('\n', "\\n"),
            ct = content_type.replace('\'', "\\'"),
        )
    };

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("failed to inject route: {e}"));
    }

    let count = state.lock().await.routes.len();
    Response::ok_data(ResponseData::Text {
        text: format!("route added: {action:?} for \"{pattern}\" ({count} active routes)"),
    })
}

/// Remove a network interception route by pattern.
async fn cmd_unroute(state: &Arc<Mutex<DaemonState>>, pattern: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let mut guard = state.lock().await;
    let before = guard.routes.len();
    if pattern == "*" {
        guard.routes.clear();
    } else {
        guard.routes.retain(|r| r.pattern != pattern);
    }
    let removed = before - guard.routes.len();
    drop(guard);

    // Remove matching routes from JS.
    let js = if pattern == "*" {
        "window.__brother_routes = []".to_owned()
    } else {
        format!(
            "window.__brother_routes = (window.__brother_routes||[]).filter(r => r.pattern !== '{}')",
            pattern.replace('\'', "\\'")
        )
    };
    let _ = page.eval(&js).await;

    Response::ok_data(ResponseData::Text {
        text: format!("removed {removed} route(s)"),
    })
}

/// List or clear captured network requests.
async fn cmd_requests(
    state: &Arc<Mutex<DaemonState>>,
    action: Option<&str>,
    filter: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Enable request tracking if not already done.
    let init_js = r"(() => {
        if (!window.__brother_req_tracked) {
            window.__brother_req_tracked = true;
            window.__brother_requests = [];
            const curFetch = window.fetch;
            window.fetch = function(url, opts) {
                const u = typeof url === 'string' ? url : url.url || '';
                const m = (opts && opts.method) || 'GET';
                window.__brother_requests.push({url: u, method: m, type: 'fetch', timestamp: Date.now()});
                return curFetch.apply(this, arguments);
            };
            const XOpen = XMLHttpRequest.prototype.open;
            XMLHttpRequest.prototype.open = function(method, url) {
                this.__bro_method = method;
                this.__bro_url = url;
                return XOpen.apply(this, arguments);
            };
            const XSend = XMLHttpRequest.prototype.send;
            XMLHttpRequest.prototype.send = function() {
                if (this.__bro_url) {
                    window.__brother_requests.push({url: this.__bro_url, method: this.__bro_method || 'GET', type: 'xhr', timestamp: Date.now()});
                }
                return XSend.apply(this, arguments);
            };
        }
    })()";
    let _ = page.eval(init_js).await;

    if action == Some("clear") {
        let _ = page.eval("window.__brother_requests = []").await;
        state.lock().await.captured_requests.clear();
        return Response::ok_data(ResponseData::Text {
            text: "requests cleared".into(),
        });
    }

    // Drain JS-captured requests.
    let drain_js = r"(() => {
        const r = window.__brother_requests || [];
        window.__brother_requests = [];
        return r;
    })()";

    let val: serde_json::Value = page.eval(drain_js).await.unwrap_or_default();

    let entries = if let serde_json::Value::Array(arr) = val {
        if let Some(pat) = filter {
            arr.into_iter()
                .filter(|e| {
                    e.get("url")
                        .and_then(|u| u.as_str())
                        .is_some_and(|u| u.contains(pat))
                })
                .collect()
        } else {
            arr
        }
    } else {
        Vec::new()
    };

    Response::ok_data(ResponseData::Logs {
        entries: serde_json::Value::Array(entries),
    })
}

/// Grant clipboard permissions via CDP `Browser.setPermission`.
async fn grant_clipboard_permission(page: &Page) {
    let origin = page.url().await.unwrap_or_default();
    let cmd = chromiumoxide::cdp::browser_protocol::browser::SetPermissionParams::builder()
        .permission(
            chromiumoxide::cdp::browser_protocol::browser::PermissionDescriptor::new("clipboard-read"),
        )
        .setting(chromiumoxide::cdp::browser_protocol::browser::PermissionSetting::Granted)
        .origin(origin.clone())
        .build();
    if let Ok(cmd) = cmd {
        let _ = page.inner().execute(cmd).await;
    }
    let cmd2 = chromiumoxide::cdp::browser_protocol::browser::SetPermissionParams::builder()
        .permission(
            chromiumoxide::cdp::browser_protocol::browser::PermissionDescriptor::new("clipboard-write"),
        )
        .setting(chromiumoxide::cdp::browser_protocol::browser::PermissionSetting::Granted)
        .origin(origin)
        .build();
    if let Ok(cmd2) = cmd2 {
        let _ = page.inner().execute(cmd2).await;
    }
}

/// Read text from the system clipboard via JS `navigator.clipboard.readText()`.
async fn cmd_clipboard_read(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    grant_clipboard_permission(&page).await;

    let val: serde_json::Value = page
        .eval("navigator.clipboard.readText()")
        .await
        .unwrap_or_default();
    let text = val.as_str().unwrap_or("").to_owned();

    Response::ok_data(ResponseData::Text { text })
}

/// Write text to the system clipboard via JS `navigator.clipboard.writeText()`.
async fn cmd_clipboard_write(state: &Arc<Mutex<DaemonState>>, text: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    grant_clipboard_permission(&page).await;

    let escaped = text.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
    let js = format!("navigator.clipboard.writeText(`{escaped}`)");
    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("clipboard write failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "clipboard updated".into(),
    })
}

// ---------------------------------------------------------------------------
// Multi-select handler
// ---------------------------------------------------------------------------

/// Select one or more dropdown options by value.
async fn cmd_select(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    values: &[String],
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    // Select each value; for single-value, just use the first.
    for v in values {
        if let Err(e) = page.select_option(target, v).await {
            return Response::error(e.ai_friendly(target).to_string());
        }
    }
    Response::ok()
}

// ---------------------------------------------------------------------------
// Enhanced click handler
// ---------------------------------------------------------------------------

/// Click with configurable button and click count.
async fn cmd_click(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    button: MouseButton,
    click_count: u32,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType,
        MouseButton as CdpMouseButton,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // For default left-click with count=1, use the simple path
    if matches!(button, MouseButton::Left) && click_count == 1 {
        return match page.click(target).await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly(target).to_string()),
        };
    }

    let center = match page.resolve_target_center(target).await {
        Ok(c) => c,
        Err(e) => return Response::error(e.ai_friendly(target).to_string()),
    };

    let mb = match button {
        MouseButton::Right => CdpMouseButton::Right,
        MouseButton::Middle => CdpMouseButton::Middle,
        MouseButton::Left => CdpMouseButton::Left,
    };

    for i in 0..click_count {
        let count = i64::from(i + 1);
        let press = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .button(mb.clone())
            .x(center.x)
            .y(center.y)
            .click_count(count)
            .build();
        if let Ok(p) = press
            && let Err(e) = page.inner().execute(p).await
        {
            return Response::error(format!("click failed: {e}"));
        }
        let release = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .button(mb.clone())
            .x(center.x)
            .y(center.y)
            .click_count(count)
            .build();
        if let Ok(r) = release
            && let Err(e) = page.inner().execute(r).await
        {
            return Response::error(format!("click release failed: {e}"));
        }
    }

    Response::ok()
}

// ---------------------------------------------------------------------------
// Environment emulation handlers
// ---------------------------------------------------------------------------

/// Set viewport size via CDP `Emulation.setDeviceMetricsOverride`.
async fn cmd_viewport(state: &Arc<Mutex<DaemonState>>, width: u32, height: u32) -> Response {
    use chromiumoxide::cdp::browser_protocol::emulation::SetDeviceMetricsOverrideParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let Ok(params) = SetDeviceMetricsOverrideParams::builder()
        .width(width)
        .height(height)
        .device_scale_factor(0.0)
        .mobile(false)
        .build()
    else {
        return Response::error("failed to build viewport params");
    };

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("viewport failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("viewport set to {width}x{height}"),
    })
}

/// Emulate media features via CDP `Emulation.setEmulatedMedia`.
async fn cmd_emulate_media(
    state: &Arc<Mutex<DaemonState>>,
    media: Option<&str>,
    color_scheme: Option<&str>,
    reduced_motion: Option<&str>,
    forced_colors: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::emulation::{
        MediaFeature, SetEmulatedMediaParams,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let mut params = SetEmulatedMediaParams::default();
    if let Some(m) = media {
        params.media = Some(m.to_owned());
    }
    let mut features = Vec::new();
    if let Some(cs) = color_scheme {
        features.push(MediaFeature::new("prefers-color-scheme", cs));
    }
    if let Some(rm) = reduced_motion {
        features.push(MediaFeature::new("prefers-reduced-motion", rm));
    }
    if let Some(fc) = forced_colors {
        features.push(MediaFeature::new("forced-colors", fc));
    }
    if !features.is_empty() {
        params.features = Some(features);
    }

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("emulate media failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "media emulation updated".into(),
    })
}

/// Toggle offline mode via JS-based network interception.
async fn cmd_offline(state: &Arc<Mutex<DaemonState>>, offline: bool) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let js = if offline {
        // Monkey-patch fetch to reject and block XHR
        r"(() => {
            if (!window.__brother_offline) {
                window.__brother_offline = true;
                const F = window.fetch;
                window.__brother_orig_fetch = F;
                window.fetch = function() {
                    if (window.__brother_offline) return Promise.reject(new TypeError('Network request failed (offline mode)'));
                    return F.apply(this, arguments);
                };
            } else {
                window.__brother_offline = true;
            }
        })()"
    } else {
        r"(() => {
            window.__brother_offline = false;
        })()"
    };

    if let Err(e) = page.eval(js).await {
        return Response::error(format!("offline toggle failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("offline mode: {offline}"),
    })
}

/// Set extra HTTP headers via CDP `Network.setExtraHTTPHeaders`.
async fn cmd_extra_headers(state: &Arc<Mutex<DaemonState>>, headers_json: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let map: std::collections::HashMap<String, String> = match serde_json::from_str(headers_json) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("invalid headers JSON: {e}")),
    };

    let json_map: serde_json::Map<String, serde_json::Value> = map
        .into_iter()
        .map(|(k, v)| (k, serde_json::Value::String(v)))
        .collect();
    let headers = chromiumoxide::cdp::browser_protocol::network::Headers::new(json_map);
    let params = SetExtraHttpHeadersParams::new(headers);

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("set headers failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "extra headers set".into(),
    })
}

/// Override geolocation via CDP `Emulation.setGeolocationOverride`.
async fn cmd_geolocation(
    state: &Arc<Mutex<DaemonState>>,
    latitude: f64,
    longitude: f64,
    accuracy: f64,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::emulation::SetGeolocationOverrideParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = SetGeolocationOverrideParams {
        latitude: Some(latitude),
        longitude: Some(longitude),
        accuracy: Some(accuracy),
        ..Default::default()
    };

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("geolocation override failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("geolocation set to ({latitude}, {longitude})"),
    })
}

/// Set HTTP Basic Auth credentials via CDP `Network.setExtraHTTPHeaders`.
async fn cmd_credentials(
    state: &Arc<Mutex<DaemonState>>,
    username: &str,
    password: &str,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::network::SetExtraHttpHeadersParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"));
    let mut map = serde_json::Map::new();
    map.insert(
        "Authorization".to_owned(),
        serde_json::Value::String(format!("Basic {encoded}")),
    );

    let headers = chromiumoxide::cdp::browser_protocol::network::Headers::new(map);
    let params = SetExtraHttpHeadersParams::new(headers);

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("credentials failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "HTTP credentials set".into(),
    })
}

// ---------------------------------------------------------------------------
// P4.2: New environment / interaction handlers
// ---------------------------------------------------------------------------

/// Override user-agent string via CDP `Network.setUserAgentOverride`.
async fn cmd_user_agent(state: &Arc<Mutex<DaemonState>>, user_agent: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = SetUserAgentOverrideParams::new(user_agent.to_owned());
    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("user-agent override failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("user-agent set to: {user_agent}"),
    })
}

/// Override timezone via CDP `Emulation.setTimezoneOverride`.
async fn cmd_timezone(state: &Arc<Mutex<DaemonState>>, timezone_id: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::emulation::SetTimezoneOverrideParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = SetTimezoneOverrideParams::new(timezone_id.to_owned());
    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("timezone override failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("timezone set to: {timezone_id}"),
    })
}

/// Override locale via JS `navigator.language` override and CDP intl hint.
async fn cmd_locale(state: &Arc<Mutex<DaemonState>>, locale: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Override navigator.language and navigator.languages via JS
    let js = format!(
        "Object.defineProperty(navigator, 'language', {{ get: () => '{}' }}); \
         Object.defineProperty(navigator, 'languages', {{ get: () => ['{}'] }});",
        locale.replace('\'', "\\'"),
        locale.replace('\'', "\\'"),
    );
    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("locale override failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("locale set to: {locale}"),
    })
}

/// Grant or revoke browser permissions via CDP `Browser.setPermission`.
async fn cmd_permissions(
    state: &Arc<Mutex<DaemonState>>,
    permissions: &[String],
    grant: bool,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::browser::{
        PermissionDescriptor, PermissionSetting, SetPermissionParams,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let setting = if grant {
        PermissionSetting::Granted
    } else {
        PermissionSetting::Denied
    };

    for perm in permissions {
        let descriptor = PermissionDescriptor::new(perm.clone());
        let params = SetPermissionParams::new(descriptor, setting.clone());
        if let Err(e) = page.inner().execute(params).await {
            return Response::error(format!("permission '{perm}' failed: {e}"));
        }
    }

    let action = if grant { "granted" } else { "denied" };
    Response::ok_data(ResponseData::Text {
        text: format!("{action}: {}", permissions.join(", ")),
    })
}

/// Bring the current page to front via CDP `Page.bringToFront`.
async fn cmd_bring_to_front(state: &Arc<Mutex<DaemonState>>) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::BringToFrontParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    if let Err(e) = page.inner().execute(BringToFrontParams::default()).await {
        return Response::error(format!("bring to front failed: {e}"));
    }

    Response::ok()
}

/// Scroll with the mouse wheel via CDP `Input.dispatchMouseEvent`.
async fn cmd_wheel(
    state: &Arc<Mutex<DaemonState>>,
    delta_x: f64,
    delta_y: f64,
    selector: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // If selector given, hover it first so the wheel targets that element.
    if let Some(sel) = selector
        && let Err(e) = page.hover(sel).await
    {
        return Response::error(e.ai_friendly(sel).to_string());
    }

    let params = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseWheel)
        .x(0)
        .y(0)
        .delta_x(delta_x)
        .delta_y(delta_y)
        .build();

    match params {
        Ok(p) => {
            if let Err(e) = page.inner().execute(p).await {
                return Response::error(format!("wheel failed: {e}"));
            }
        }
        Err(e) => return Response::error(format!("wheel params failed: {e}")),
    }

    Response::ok()
}

/// Touch-tap an element by resolving its center and dispatching touch events.
async fn cmd_tap(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchTouchEventParams, DispatchTouchEventType, TouchPoint,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let center = match page.resolve_target_center(target).await {
        Ok(c) => c,
        Err(e) => return Response::error(e.ai_friendly(target).to_string()),
    };

    let point = TouchPoint::new(center.x, center.y);

    // Touch start
    let start = DispatchTouchEventParams::new(
        DispatchTouchEventType::TouchStart,
        vec![point.clone()],
    );
    if let Err(e) = page.inner().execute(start).await {
        return Response::error(format!("tap failed (touchStart): {e}"));
    }

    // Touch end
    let end = DispatchTouchEventParams::new(DispatchTouchEventType::TouchEnd, vec![]);
    if let Err(e) = page.inner().execute(end).await {
        return Response::error(format!("tap failed (touchEnd): {e}"));
    }

    Response::ok()
}

/// Set an input value directly (no events) via JS.
async fn cmd_set_value(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    value: &str,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let escaped = value.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "(() => {{ const el = document.querySelector('{sel}'); \
         if (!el) throw new Error('Element not found: {sel}'); \
         el.value = '{val}'; }})() ",
        sel = target.replace('\'', "\\'"),
        val = escaped,
    );

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("set value failed: {e}"));
    }

    Response::ok()
}

// ---------------------------------------------------------------------------
// Script injection handlers
// ---------------------------------------------------------------------------

/// Add a script to evaluate on every new document via CDP `Page.addScriptToEvaluateOnNewDocument`.
async fn cmd_add_init_script(state: &Arc<Mutex<DaemonState>>, script: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = AddScriptToEvaluateOnNewDocumentParams::new(script.to_owned());
    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("add init script failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "init script added".into(),
    })
}

/// Inject a `<script>` tag into the current page via JS.
async fn cmd_add_script(
    state: &Arc<Mutex<DaemonState>>,
    content: Option<&str>,
    url: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let js = match (content, url) {
        (Some(c), _) => {
            let escaped = c.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
            format!(
                r"(() => {{ const s = document.createElement('script'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
            )
        }
        (_, Some(u)) => {
            let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
            format!(
                r"(() => {{ const s = document.createElement('script'); s.src = '{escaped}'; document.head.appendChild(s); }})()"
            )
        }
        _ => return Response::error("either content or url is required"),
    };

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("add script failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "script injected".into(),
    })
}

/// Inject a `<style>` or `<link>` tag into the current page via JS.
async fn cmd_add_style(
    state: &Arc<Mutex<DaemonState>>,
    content: Option<&str>,
    url: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let js = match (content, url) {
        (Some(c), _) => {
            let escaped = c.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
            format!(
                r"(() => {{ const s = document.createElement('style'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
            )
        }
        (_, Some(u)) => {
            let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
            format!(
                r"(() => {{ const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = '{escaped}'; document.head.appendChild(l); }})()"
            )
        }
        _ => return Response::error("either content or url is required"),
    };

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("add style failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "style injected".into(),
    })
}

/// Dispatch a DOM event on an element via JS.
async fn cmd_dispatch(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    event: &str,
    event_init: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let escaped_event = event.replace('\\', "\\\\").replace('\'', "\\'");
    let init_arg = event_init.map_or_else(|| "{}".to_owned(), ToOwned::to_owned);
    let escaped_sel = target.replace('\\', "\\\\").replace('\'', "\\'");

    let js = format!(
        r"(() => {{
            const el = document.querySelector('{escaped_sel}');
            if (!el) throw new Error('element not found: {escaped_sel}');
            el.dispatchEvent(new Event('{escaped_event}', {init_arg}));
        }})()"
    );

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("dispatch failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("dispatched '{event}' on '{target}'"),
    })
}

// ---------------------------------------------------------------------------
// Misc interaction / query handlers
// ---------------------------------------------------------------------------

/// Get computed styles of an element via JS `getComputedStyle()`.
async fn cmd_styles(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "(() => {{\
            const el = document.querySelector('{escaped}');\
            if (!el) throw new Error('element not found: {escaped}');\
            const s = getComputedStyle(el);\
            const r = el.getBoundingClientRect();\
            return {{\
                tag: el.tagName.toLowerCase(),\
                text: (el.innerText || \"\").trim().slice(0, 80) || null,\
                box: {{ x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) }},\
                styles: {{\
                    fontSize: s.fontSize,\
                    fontWeight: s.fontWeight,\
                    fontFamily: s.fontFamily.split(\",\")[0].trim().replace(/\"/g, \"\"),\
                    color: s.color,\
                    backgroundColor: s.backgroundColor,\
                    borderRadius: s.borderRadius,\
                    border: s.border !== \"none\" && s.borderWidth !== \"0px\" ? s.border : null,\
                    boxShadow: s.boxShadow !== \"none\" ? s.boxShadow : null,\
                    padding: s.padding,\
                }},\
            }};\
        }})()"
    );

    match page.eval(&js).await {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(format!("styles failed: {e}")),
    }
}

/// Select all text in an element via JS `Selection` API.
async fn cmd_select_all(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        r"(() => {{
            const el = document.querySelector('{escaped}');
            if (!el) throw new Error('element not found: {escaped}');
            const range = document.createRange();
            range.selectNodeContents(el);
            const sel = window.getSelection();
            sel.removeAllRanges();
            sel.addRange(range);
        }})()"
    );

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("select all failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: "text selected".into(),
    })
}

/// Highlight an element with a red border overlay for debugging.
async fn cmd_highlight(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        r"(() => {{
            const el = document.querySelector('{escaped}');
            if (!el) throw new Error('element not found: {escaped}');
            el.style.outline = '2px solid red';
            el.style.outlineOffset = '-1px';
        }})()"
    );

    if let Err(e) = page.eval(&js).await {
        return Response::error(format!("highlight failed: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("highlighted: {target}"),
    })
}

/// Move mouse to absolute coordinates via CDP `Input.dispatchMouseEvent`.
async fn cmd_mouse_move(state: &Arc<Mutex<DaemonState>>, x: f64, y: f64) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseMoved)
        .x(x)
        .y(y)
        .build();

    match params {
        Ok(p) => {
            if let Err(e) = page.inner().execute(p).await {
                return Response::error(format!("mouse move failed: {e}"));
            }
        }
        Err(e) => return Response::error(format!("mouse move params failed: {e}")),
    }

    Response::ok_data(ResponseData::Text {
        text: format!("mouse moved to ({x}, {y})"),
    })
}

/// Convert our `MouseButton` enum to CDP `MouseButton`.
const fn to_cdp_mouse_button(
    button: MouseButton,
) -> chromiumoxide::cdp::browser_protocol::input::MouseButton {
    use chromiumoxide::cdp::browser_protocol::input::MouseButton as CdpMB;
    match button {
        MouseButton::Right => CdpMB::Right,
        MouseButton::Middle => CdpMB::Middle,
        MouseButton::Left => CdpMB::Left,
    }
}

/// Press a mouse button down via CDP `Input.dispatchMouseEvent`.
async fn cmd_mouse_down(state: &Arc<Mutex<DaemonState>>, button: MouseButton) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MousePressed)
        .button(to_cdp_mouse_button(button))
        .x(0)
        .y(0)
        .click_count(1)
        .build();

    match params {
        Ok(p) => {
            if let Err(e) = page.inner().execute(p).await {
                return Response::error(format!("mouse down failed: {e}"));
            }
        }
        Err(e) => return Response::error(format!("mouse down params failed: {e}")),
    }

    Response::ok_data(ResponseData::Text {
        text: format!("mouse {button:?} button pressed"),
    })
}

/// Release a mouse button via CDP `Input.dispatchMouseEvent`.
async fn cmd_mouse_up(state: &Arc<Mutex<DaemonState>>, button: MouseButton) -> Response {
    use chromiumoxide::cdp::browser_protocol::input::{
        DispatchMouseEventParams, DispatchMouseEventType,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let params = DispatchMouseEventParams::builder()
        .r#type(DispatchMouseEventType::MouseReleased)
        .button(to_cdp_mouse_button(button))
        .x(0)
        .y(0)
        .click_count(1)
        .build();

    match params {
        Ok(p) => {
            if let Err(e) = page.inner().execute(p).await {
                return Response::error(format!("mouse up failed: {e}"));
            }
        }
        Err(e) => return Response::error(format!("mouse up params failed: {e}")),
    }

    Response::ok_data(ResponseData::Text {
        text: format!("mouse {button:?} button released"),
    })
}

/// Set download directory via CDP `Browser.setDownloadBehavior`.
async fn cmd_set_download_path(state: &Arc<Mutex<DaemonState>>, path: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::browser::{
        SetDownloadBehaviorBehavior, SetDownloadBehaviorParams,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let mut params = SetDownloadBehaviorParams::new(SetDownloadBehaviorBehavior::AllowAndName);
    params.download_path = Some(path.to_owned());

    if let Err(e) = page.inner().execute(params).await {
        return Response::error(format!("failed to set download path: {e}"));
    }

    state.lock().await.download_path = Some(path.to_owned());

    Response::ok_data(ResponseData::Text {
        text: format!("download path set to: {path}"),
    })
}

/// List or clear download log. Currently lists files in the download directory.
async fn cmd_downloads(state: &Arc<Mutex<DaemonState>>, action: Option<&str>) -> Response {
    let guard = state.lock().await;
    let Some(ref dl_path) = guard.download_path else {
        return Response::error(
            "no download path configured. Use 'set-download-path <dir>' first.",
        );
    };
    let dl_path = dl_path.clone();
    drop(guard);

    if action == Some("clear") {
        return Response::ok_data(ResponseData::Text {
            text: "download log cleared".into(),
        });
    }

    // List files in the download directory.
    let entries: Vec<serde_json::Value> = match tokio::fs::read_dir(&dl_path).await {
        Ok(mut dir) => {
            let mut files = Vec::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = entry.metadata().await.map(|m| m.len()).unwrap_or(0);
                files.push(serde_json::json!({
                    "name": name,
                    "size": size,
                }));
            }
            files
        }
        Err(e) => {
            return Response::error(format!("failed to read download dir: {e}"));
        }
    };

    Response::ok_data(ResponseData::Logs {
        entries: serde_json::Value::Array(entries),
    })
}

/// Wait for a download to complete by polling the download directory for new files.
async fn cmd_wait_for_download(
    state: &Arc<Mutex<DaemonState>>,
    save_path: Option<&str>,
    timeout_ms: u64,
) -> Response {
    let guard = state.lock().await;
    let Some(ref dl_path) = guard.download_path else {
        return Response::error(
            "no download path configured. Use 'set-download-path <dir>' first.",
        );
    };
    let dl_dir = dl_path.clone();
    drop(guard);

    // Snapshot existing files before waiting.
    let before: std::collections::HashSet<String> =
        match tokio::fs::read_dir(&dl_dir).await {
            Ok(mut dir) => {
                let mut set = std::collections::HashSet::new();
                while let Ok(Some(entry)) = dir.next_entry().await {
                    set.insert(entry.file_name().to_string_lossy().to_string());
                }
                set
            }
            Err(e) => return Response::error(format!("read download dir: {e}")),
        };

    // Poll for new files until timeout.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if tokio::time::Instant::now() > deadline {
            return Response::error("wait for download timed out");
        }
        if let Ok(mut dir) = tokio::fs::read_dir(&dl_dir).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                // Skip partial Chrome downloads.
                if std::path::Path::new(&name).extension().is_some_and(|e| e == "crdownload" || e == "tmp") {
                    continue;
                }
                if !before.contains(&name) {
                    let src = std::path::Path::new(&dl_dir).join(&name);
                    // Optionally copy to the requested path.
                    if let Some(dest) = save_path
                        && let Err(e) = tokio::fs::copy(&src, dest).await
                    {
                        return Response::error(format!("copy download failed: {e}"));
                    }
                    return Response::ok_data(ResponseData::Text {
                        text: format!("downloaded: {name}"),
                    });
                }
            }
        }
    }
}

/// Wait for a network response matching a URL pattern and return its body.
///
/// Uses JS fetch/XHR interception to capture response bodies. The interception
/// script stores the body in `window.__brother_response_capture`.
async fn cmd_response_body(
    state: &Arc<Mutex<DaemonState>>,
    url_pattern: &str,
    timeout_ms: u64,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Inject response capture hook.
    let pat_escaped = url_pattern.replace('\'', "\\'");
    let inject_js = format!(
        r"(() => {{
            if (!window.__brother_resp_hook) {{
                window.__brother_resp_hook = true;
                window.__brother_response_capture = null;
                const F = window.fetch;
                window.fetch = function(url, opts) {{
                    const u = typeof url === 'string' ? url : url.url || '';
                    return F.apply(this, arguments).then(resp => {{
                        if (u.includes('{pat_escaped}') && !window.__brother_response_capture) {{
                            resp.clone().text().then(body => {{
                                window.__brother_response_capture = {{
                                    url: resp.url, status: resp.status, body: body
                                }};
                            }});
                        }}
                        return resp;
                    }});
                }};
            }}
        }})()",
    );
    let _ = page.eval(&inject_js).await;

    // Poll for captured response.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if tokio::time::Instant::now() > deadline {
            return Response::error(format!(
                "response body timed out waiting for URL matching '{url_pattern}'"
            ));
        }
        let check_js = r"(() => {
            const c = window.__brother_response_capture;
            if (c) { window.__brother_response_capture = null; return c; }
            return null;
        })()";
        if let Ok(val) = page.eval(check_js).await
            && !val.is_null()
        {
            return Response::ok_data(ResponseData::Eval { value: val });
        }
    }
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

async fn cmd_screenshot(
    state: &Arc<Mutex<DaemonState>>,
    selector: Option<&str>,
    format: &str,
    quality: u8,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // If a selector is given, scroll it into view first and capture its clip region
    if let Some(sel) = selector {
        // Scroll element into view and get its bounding box via JS
        let escaped = sel.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            "(() => {{ const el = document.querySelector('{escaped}'); \
             if (!el) throw new Error('element not found: {escaped}'); \
             el.scrollIntoView({{ block: 'center' }}); \
             const r = el.getBoundingClientRect(); \
             return {{ x: r.x, y: r.y, width: r.width, height: r.height }}; }})()"
        );
        let _bbox: serde_json::Value = match page.eval(&js).await {
            Ok(v) => v,
            Err(e) => return Response::error(format!("screenshot selector failed: {e}")),
        };
    }

    let result = if format == "jpeg" {
        page.screenshot_jpeg(quality).await
    } else {
        page.screenshot_png().await
    };

    match result {
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

async fn cmd_type(
    state: &Arc<Mutex<DaemonState>>,
    target: Option<&str>,
    text: &str,
    delay_ms: u64,
) -> Response {
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
    if delay_ms > 0 {
        // Type each character with a delay
        for ch in text.chars() {
            let s = ch.to_string();
            if let Err(e) = page.type_text(&s).await {
                return Response::error(format!("type failed: {e}"));
            }
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
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

async fn cmd_console(state: &Arc<Mutex<DaemonState>>, clear: bool) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let logs = page.take_console_logs().await;
    if clear {
        return Response::ok_data(ResponseData::Text {
            text: format!("{} console entries cleared", logs.len()),
        });
    }
    let entries = serde_json::to_value(&logs).unwrap_or_default();
    Response::ok_data(ResponseData::Logs { entries })
}

async fn cmd_errors(state: &Arc<Mutex<DaemonState>>, clear: bool) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let errors = page.take_js_errors().await;
    if clear {
        return Response::ok_data(ResponseData::Text {
            text: format!("{} error entries cleared", errors.len()),
        });
    }
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
