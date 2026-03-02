//! Daemon server — long-running process that holds the browser instance.
//!
//! Listens on `127.0.0.1:<port>`, accepts newline-delimited JSON
//! [`Request`](crate::protocol::Request) messages, and returns
//! [`Response`](crate::protocol::Response) messages. The browser is lazily
//! launched on first command.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use brother::{Browser, BrowserConfig, Error, Page};
use futures::StreamExt;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::protocol::{Request, Response, ResponseData, RouteAction, WaitCondition, WaitStrategy};

/// Default idle timeout before auto-shutdown.
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_mins(5);

/// Shared state across connections.
struct DaemonState {
    browser: Option<Browser>,
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
pub async fn run(idle_timeout: Option<Duration>) -> brother::Result<()> {
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

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

#[allow(
    clippy::cognitive_complexity,
    clippy::too_many_lines,
    clippy::large_stack_frames
)]
async fn dispatch(req: Request, state: &Arc<Mutex<DaemonState>>) -> Response {
    match req {
        // -- Connection -------------------------------------------------------
        Request::Connect { target } => cmd_connect(state, &target).await,

        // -- Navigation -------------------------------------------------------
        Request::Navigate { url, wait } => cmd_navigate(state, &url, wait).await,
        Request::Back => page_ok!(state, go_back()),
        Request::Forward => page_ok!(state, go_forward()),
        Request::Reload => page_ok!(state, reload()),

        // -- Observation ------------------------------------------------------
        Request::Snapshot { options } => cmd_snapshot(state, options).await,
        Request::Screenshot {
            full_page,
            selector,
            format,
            quality,
        } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page
                .screenshot(full_page, selector.as_deref(), &format, Some(quality))
                .await
            {
                Ok(bytes) => {
                    let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    Response::ok_data(ResponseData::Screenshot { data })
                }
                Err(e) => Response::error(format!("screenshot failed: {e}")),
            }
        }
        Request::Eval { expression } => page_eval!(state, eval(&expression)),

        // -- Interaction ------------------------------------------------------
        Request::Click {
            target,
            button,
            click_count,
        } => {
            page_ok!(state, &target, click_with(&target, button, click_count))
        }
        Request::DblClick { target } => page_ok!(state, &target, dblclick(&target)),
        Request::Fill { target, value } => page_ok!(state, &target, fill(&target, &value)),
        Request::Type {
            target,
            text,
            delay_ms,
        } => {
            page_ok!(state, type_with_delay(target.as_deref(), &text, delay_ms))
        }
        Request::Press { key } => page_ok!(state, key_press(&key)),
        Request::Select { target, values } => {
            page_ok!(state, &target, select_options(&target, &values))
        }
        Request::Check { target } => page_ok!(state, &target, check(&target)),
        Request::Uncheck { target } => page_ok!(state, &target, uncheck(&target)),
        Request::Hover { target } => page_ok!(state, &target, hover(&target)),
        Request::Focus { target } => page_ok!(state, &target, focus(&target)),
        Request::Scroll {
            direction,
            pixels,
            target,
        } => {
            page_ok!(state, scroll(direction, pixels, target.as_deref()))
        }
        Request::SetValue { target, value } => {
            page_ok!(state, &target, set_value(&target, &value))
        }

        // -- Frame (iframe) ---------------------------------------------------
        Request::Frame { selector } => cmd_frame(state, &selector).await,
        Request::MainFrame => cmd_main_frame(state).await,

        // -- Raw keyboard -----------------------------------------------------
        Request::KeyDown { key } => page_ok!(state, key_down(&key)),
        Request::KeyUp { key } => page_ok!(state, key_up(&key)),
        Request::InsertText { text } => page_ok!(state, insert_text(&text)),

        // -- File / DOM -------------------------------------------------------
        Request::Upload { target, files } => page_ok!(state, &target, upload(&target, &files)),
        Request::Drag { source, target } => page_ok!(state, &source, drag(&source, &target)),
        Request::Clear { target } => page_ok!(state, &target, clear(&target)),
        Request::ScrollIntoView { target } => {
            page_ok!(state, &target, scroll_into_view(&target))
        }
        Request::BoundingBox { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.bounding_box(&target).await {
                Ok((x, y, w, h)) => Response::ok_data(ResponseData::BoundingBox {
                    x,
                    y,
                    width: w,
                    height: h,
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::SetContent { html } => page_ok!(state, set_content(&html)),
        Request::Pdf { path } => page_ok!(state, pdf(&path)),

        // -- Network interception ---------------------------------------------
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

        // -- Download ---------------------------------------------------------
        Request::SetDownloadPath { path } => cmd_set_download_path(state, &path).await,
        Request::Downloads { action } => cmd_downloads(state, action.as_deref()).await,
        Request::WaitForDownload { path, timeout_ms } => {
            cmd_wait_for_download(state, path.as_deref(), timeout_ms).await
        }
        Request::ResponseBody { url, timeout_ms } => {
            cmd_response_body(state, &url, timeout_ms).await
        }

        // -- Clipboard --------------------------------------------------------
        Request::ClipboardRead => page_text!(state, clipboard_read()),
        Request::ClipboardWrite { text } => page_ok!(state, clipboard_write(&text)),

        // -- Environment emulation --------------------------------------------
        Request::Viewport { width, height } => page_ok!(state, set_viewport(width, height)),
        Request::EmulateMedia {
            media,
            color_scheme,
            reduced_motion,
            forced_colors,
        } => {
            page_ok!(
                state,
                emulate_media(
                    media.as_deref(),
                    color_scheme.as_deref(),
                    reduced_motion.as_deref(),
                    forced_colors.as_deref(),
                )
            )
        }
        Request::Offline { offline } => page_ok!(state, set_offline(offline)),
        Request::ExtraHeaders { headers_json } => {
            let map: serde_json::Map<String, serde_json::Value> =
                match serde_json::from_str(&headers_json) {
                    Ok(m) => m,
                    Err(e) => return Response::error(format!("invalid headers JSON: {e}")),
                };
            page_ok!(state, set_extra_headers(map))
        }
        Request::Geolocation {
            latitude,
            longitude,
            accuracy,
        } => {
            page_ok!(state, set_geolocation(latitude, longitude, accuracy))
        }
        Request::Credentials { username, password } => {
            page_ok!(state, set_credentials(&username, &password))
        }
        Request::UserAgent { user_agent } => page_ok!(state, set_user_agent(&user_agent)),
        Request::Timezone { timezone_id } => page_ok!(state, set_timezone(&timezone_id)),
        Request::Locale { locale } => page_ok!(state, set_locale(&locale)),
        Request::Permissions { permissions, grant } => {
            page_ok!(state, set_permissions(&permissions, grant))
        }
        Request::BringToFront => page_ok!(state, bring_to_front()),

        // -- Script injection -------------------------------------------------
        Request::AddInitScript { script } => page_ok!(state, add_init_script(&script)),
        Request::AddScript { content, url } => {
            page_ok!(state, add_script(content.as_deref(), url.as_deref()))
        }
        Request::AddStyle { content, url } => {
            page_ok!(state, add_style(content.as_deref(), url.as_deref()))
        }
        Request::Dispatch {
            target,
            event,
            event_init,
        } => {
            page_ok!(
                state,
                dispatch_event(&target, &event, event_init.as_deref())
            )
        }

        // -- Misc interaction / queries ---------------------------------------
        Request::Styles { target } => page_eval!(state, get_styles(&target)),
        Request::SelectAll { target } => page_ok!(state, select_all_text(&target)),
        Request::Highlight { target } => page_ok!(state, &target, highlight(&target)),
        Request::MouseMove { x, y } => page_ok!(state, mouse_move(x, y)),
        Request::MouseDown { button } => page_ok!(state, mouse_down(button)),
        Request::MouseUp { button } => page_ok!(state, mouse_up(button)),
        Request::Wheel {
            delta_x,
            delta_y,
            selector,
        } => {
            page_ok!(state, wheel(delta_x, delta_y, selector.as_deref()))
        }
        Request::Tap { target } => page_ok!(state, &target, tap(&target)),

        // -- Query ------------------------------------------------------------
        Request::GetText { target } => page_text!(state, get_text(target.as_deref())),
        Request::GetUrl => page_text!(state, url()),
        Request::GetTitle => page_text!(state, title()),
        Request::GetHtml { target } => page_text!(state, &target, get_html(&target)),
        Request::GetValue { target } => page_text!(state, &target, get_value(&target)),
        Request::GetAttribute { target, attribute } => {
            page_text!(state, &target, get_attribute(&target, &attribute))
        }

        // -- State checks -----------------------------------------------------
        Request::IsVisible { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_visible(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::IsEnabled { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_enabled(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::IsChecked { target } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.is_checked(&target).await {
                Ok(val) => Response::ok_data(ResponseData::Text {
                    text: val.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&target).to_string()),
            }
        }
        Request::Count { selector } => {
            let page = match get_page(state).await {
                Ok(p) => p,
                Err(r) => return r,
            };
            match page.count(&selector).await {
                Ok(n) => Response::ok_data(ResponseData::Text {
                    text: n.to_string(),
                }),
                Err(e) => Response::error(e.ai_friendly(&selector).to_string()),
            }
        }

        // -- Wait -------------------------------------------------------------
        Request::Wait { condition } => cmd_wait(state, condition).await,

        // -- Dialog -----------------------------------------------------------
        Request::DialogMessage => {
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
        Request::DialogAccept { prompt_text } => {
            page_ok!(state, dialog_accept(prompt_text.as_deref()))
        }
        Request::DialogDismiss => page_ok!(state, dialog_dismiss()),

        // -- Cookie / Storage -------------------------------------------------
        Request::GetCookies => page_eval!(state, get_cookies()),
        Request::SetCookie { cookie } => page_ok!(state, set_cookie(&cookie)),
        Request::ClearCookies => page_ok!(state, clear_cookies()),
        Request::GetStorage { key, session } => page_text!(state, get_storage(&key, session)),
        Request::SetStorage {
            key,
            value,
            session,
        } => {
            page_ok!(state, set_storage(&key, &value, session))
        }
        Request::ClearStorage { session } => page_ok!(state, clear_storage(session)),

        // -- Tab management ---------------------------------------------------
        Request::TabNew { url } => cmd_tab_new(state, url.as_deref()).await,
        Request::TabList => cmd_tab_list(state).await,
        Request::TabSelect { index } => cmd_tab_select(state, index).await,
        Request::TabClose { index } => cmd_tab_close(state, index).await,

        // -- Debug ------------------------------------------------------------
        Request::Console { clear } => {
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
            Response::ok_data(ResponseData::Logs {
                entries: serde_json::to_value(&logs).unwrap_or_default(),
            })
        }
        Request::Errors { clear } => {
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
            Response::ok_data(ResponseData::Logs {
                entries: serde_json::to_value(&errors).unwrap_or_default(),
            })
        }

        // -- Lifecycle --------------------------------------------------------
        Request::Status => {
            let guard = state.lock().await;
            let browser_running = guard.browser.is_some();
            let page_url = if let Some(page) = guard.pages.get(guard.active_tab) {
                page.url().await.ok()
            } else {
                None
            };
            Response::ok_data(ResponseData::Status {
                browser_running,
                page_url,
            })
        }
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

async fn launch_browser(config: BrowserConfig) -> brother::Result<(Browser, Page)> {
    let (browser, mut handler) = Browser::launch(config).await?;
    tokio::spawn(async move { while handler.next().await.is_some() {} });
    let page = browser.new_blank_page().await?;
    Ok((browser, page))
}

/// Connect to an existing browser via CDP websocket URL or debugging port.
async fn cmd_connect(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
    let ws_url = resolve_cdp_endpoint(target).await;
    let ws_url = match ws_url {
        Ok(url) => url,
        Err(e) => return Response::error(format!("failed to resolve CDP endpoint: {e}")),
    };

    let connect_result = Browser::connect(&ws_url).await;
    let (browser, mut handler) = match connect_result {
        Ok(pair) => pair,
        Err(e) => return Response::error(format!("connect failed: {e}")),
    };
    tokio::spawn(async move { while handler.next().await.is_some() {} });

    let pages_result = browser.pages().await;
    let pages = match pages_result {
        Ok(p) if !p.is_empty() => p,
        Ok(_) => match browser.new_blank_page().await {
            Ok(p) => vec![p],
            Err(e) => return Response::error(format!("connected but no pages: {e}")),
        },
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
async fn resolve_cdp_endpoint(target: &str) -> brother::Result<String> {
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
    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| Error::Browser(format!("invalid JSON from {version_url}: {e}")))?;
    body["webSocketDebuggerUrl"]
        .as_str()
        .map(String::from)
        .ok_or_else(|| {
            Error::Browser("webSocketDebuggerUrl not found in /json/version response".into())
        })
}

/// Switch execution context to a child frame.
async fn cmd_frame(state: &Arc<Mutex<DaemonState>>, selector: &str) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::GetFrameTreeParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let tree_resp = match page.inner().execute(GetFrameTreeParams::default()).await {
        Ok(r) => r,
        Err(e) => return Response::error(format!("failed to get frame tree: {e}")),
    };

    let root = &tree_resp.result.frame_tree;
    let children = root.child_frames.as_deref().unwrap_or_default();

    if children.is_empty() {
        return Response::error("no child frames found on this page");
    }

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

    state.lock().await.routes.push(InterceptRoute {
        pattern: pattern.clone(),
        action,
        status,
        body: body.clone(),
        content_type: content_type.clone(),
    });

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

// ---------------------------------------------------------------------------
// Download handlers (require DaemonState)
// ---------------------------------------------------------------------------

/// Set download directory via CDP and store path in `DaemonState`.
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

/// List or clear download log.
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

    let entries: Vec<serde_json::Value> = match tokio::fs::read_dir(&dl_path).await {
        Ok(mut dir) => {
            let mut files = Vec::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                let size = entry.metadata().await.map_or(0, |m| m.len());
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

    let before: std::collections::HashSet<String> = match tokio::fs::read_dir(&dl_dir).await {
        Ok(mut dir) => {
            let mut set = std::collections::HashSet::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                set.insert(entry.file_name().to_string_lossy().to_string());
            }
            set
        }
        Err(e) => return Response::error(format!("read download dir: {e}")),
    };

    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if tokio::time::Instant::now() > deadline {
            return Response::error("wait for download timed out");
        }
        if let Ok(mut dir) = tokio::fs::read_dir(&dl_dir).await {
            while let Ok(Some(entry)) = dir.next_entry().await {
                let name = entry.file_name().to_string_lossy().to_string();
                if std::path::Path::new(&name)
                    .extension()
                    .is_some_and(|e| e == "crdownload" || e == "tmp")
                {
                    continue;
                }
                if !before.contains(&name) {
                    let src = std::path::Path::new(&dl_dir).join(&name);
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
async fn cmd_response_body(
    state: &Arc<Mutex<DaemonState>>,
    url_pattern: &str,
    timeout_ms: u64,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

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
// Handlers requiring structured responses or complex logic
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
    options: brother::SnapshotOptions,
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
    if guard.active_tab >= guard.pages.len() {
        guard.active_tab = guard.pages.len() - 1;
    }
    Response::ok_data(ResponseData::Text {
        text: format!("tab {idx} closed, active tab: {}", guard.active_tab),
    })
}

// ---------------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------------

async fn write_port_file(port: u16) -> brother::Result<()> {
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

async fn write_pid_file() -> brother::Result<()> {
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
