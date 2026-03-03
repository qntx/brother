//! Command handlers that require complex logic beyond simple macro dispatch.
//!
//! Large handler groups are split into submodules:
//! - [`diff`]: Snapshot diff, screenshot diff, URL diff, PNG utilities.
//! - [`state`]: State persistence (save/load/list/clear/show/clean/rename).
//! - [`trace`]: CDP tracing, profiler, and domain filter.

mod diff;
mod state;
mod trace;

pub(super) use diff::{cmd_diff_screenshot, cmd_diff_snapshot, cmd_diff_url};
pub(super) use state::{
    cmd_state_clean, cmd_state_clear, cmd_state_list, cmd_state_load, cmd_state_rename,
    cmd_state_save, cmd_state_show,
};
pub(super) use trace::{
    cmd_profiler_start, cmd_profiler_stop, cmd_set_allowed_domains, cmd_trace_start, cmd_trace_stop,
};

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use brother::{Browser, Error, MouseButton};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData, RouteAction, WaitCondition, WaitStrategy};

use super::{DaemonState, ensure_browser, get_page};

pub(super) async fn cmd_navigate(
    state: &Arc<Mutex<DaemonState>>,
    url: &str,
    wait: WaitStrategy,
) -> Response {
    // Domain filter: block navigation to non-allowed domains.
    {
        let guard = state.lock().await;
        if !guard.allowed_domains.is_empty()
            && let Some(host) = super::domain_filter::extract_host(url)
            && !super::domain_filter::is_allowed(&host, &guard.allowed_domains)
        {
            return Response::error(format!(
                "navigation blocked: {host} is not in the allowed domains list"
            ));
        }
    }
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

pub(super) async fn cmd_snapshot(
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

pub(super) async fn cmd_wait(
    state: &Arc<Mutex<DaemonState>>,
    condition: WaitCondition,
) -> Response {
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

/// Connect to an existing browser via CDP websocket URL or debugging port.
pub(super) async fn cmd_connect(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
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
pub(super) async fn cmd_frame(state: &Arc<Mutex<DaemonState>>, selector: &str) -> Response {
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
pub(super) async fn cmd_main_frame(state: &Arc<Mutex<DaemonState>>) -> Response {
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
pub(super) async fn cmd_route(
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

    state.lock().await.routes.push(pattern.clone());

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
pub(super) async fn cmd_unroute(state: &Arc<Mutex<DaemonState>>, pattern: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let mut guard = state.lock().await;
    let before = guard.routes.len();
    if pattern == "*" {
        guard.routes.clear();
    } else {
        guard.routes.retain(|r| r != pattern);
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
pub(super) async fn cmd_requests(
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

/// Set download directory via CDP and store path in `DaemonState`.
pub(super) async fn cmd_set_download_path(state: &Arc<Mutex<DaemonState>>, path: &str) -> Response {
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
pub(super) async fn cmd_downloads(
    state: &Arc<Mutex<DaemonState>>,
    action: Option<&str>,
) -> Response {
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
pub(super) async fn cmd_wait_for_download(
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

/// Click an element and wait for the resulting download to complete.
///
/// Automatically sets up a temp download directory if none is configured.
pub(super) async fn cmd_download(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    save_path: Option<&str>,
    timeout_ms: u64,
) -> Response {
    // Ensure a download directory is configured.
    {
        let guard = state.lock().await;
        if guard.download_path.is_none() {
            drop(guard);
            let tmp = std::env::temp_dir().join("brother-downloads");
            let _ = tokio::fs::create_dir_all(&tmp).await;
            let resp = cmd_set_download_path(state, tmp.to_string_lossy().as_ref()).await;
            if matches!(resp, Response::Error { .. }) {
                return resp;
            }
        }
    }

    // Snapshot existing files before click.
    let dl_dir = state.lock().await.download_path.clone().unwrap_or_default();
    let before: std::collections::HashSet<String> = match tokio::fs::read_dir(&dl_dir).await {
        Ok(mut dir) => {
            let mut set = std::collections::HashSet::new();
            while let Ok(Some(entry)) = dir.next_entry().await {
                set.insert(entry.file_name().to_string_lossy().to_string());
            }
            set
        }
        Err(_) => std::collections::HashSet::new(),
    };

    // Click the element.
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.click(target).await {
        return Response::error(e.ai_friendly(target).to_string());
    }

    // Wait for a new file to appear.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        if tokio::time::Instant::now() > deadline {
            return Response::error("download timed out");
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
                    let final_path = save_path.unwrap_or(&name);
                    return Response::ok_data(ResponseData::Text {
                        text: format!("downloaded: {final_path}"),
                    });
                }
            }
        }
    }
}

/// Wait for a network response matching a URL pattern and return its body.
pub(super) async fn cmd_response_body(
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

pub(super) async fn cmd_window_new(
    state: &Arc<Mutex<DaemonState>>,
    width: Option<u32>,
    height: Option<u32>,
) -> Response {
    ensure_browser(state).await.ok();
    let mut guard = state.lock().await;
    let Some(ref browser) = guard.browser else {
        return Response::error("no browser running");
    };
    // Create a new page (new window in headed mode, new tab in headless)
    match browser.new_page("about:blank").await {
        Ok(page) => {
            // Set viewport if dimensions provided
            if let (Some(w), Some(h)) = (width, height) {
                let _ = page.set_viewport(w, h).await;
            }
            guard.pages.push(page);
            guard.active_tab = guard.pages.len() - 1;
            Response::ok_data(ResponseData::Text {
                text: format!("window {} opened", guard.active_tab),
            })
        }
        Err(e) => Response::error(format!("window new failed: {e}")),
    }
}

pub(super) async fn cmd_tab_new(state: &Arc<Mutex<DaemonState>>, url: Option<&str>) -> Response {
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

pub(super) async fn cmd_tab_list(state: &Arc<Mutex<DaemonState>>) -> Response {
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

pub(super) async fn cmd_tab_select(state: &Arc<Mutex<DaemonState>>, index: usize) -> Response {
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

pub(super) async fn cmd_tab_close(
    state: &Arc<Mutex<DaemonState>>,
    index: Option<usize>,
) -> Response {
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
// Diff, State, and Trace/Profiler handlers are in submodules:
// see `diff.rs`, `state.rs`, `trace.rs` in the `handlers/` directory.

pub(super) async fn cmd_har_start(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Enable Network domain to capture requests
    let js = r"(() => {
        if (!window.__brother_har) {
            window.__brother_har = [];
            const origFetch = window.fetch;
            window.fetch = function(...args) {
                const url = typeof args[0] === 'string' ? args[0] : args[0]?.url || '';
                const method = args[1]?.method || 'GET';
                const start = Date.now();
                return origFetch.apply(this, args).then(resp => {
                    window.__brother_har.push({
                        startedDateTime: new Date(start).toISOString(),
                        request: { method, url },
                        response: { status: resp.status, statusText: resp.statusText },
                        time: Date.now() - start
                    });
                    return resp;
                });
            };
            const origXHR = XMLHttpRequest.prototype.open;
            XMLHttpRequest.prototype.open = function(method, url) {
                this.__har_method = method;
                this.__har_url = url;
                this.__har_start = Date.now();
                this.addEventListener('load', function() {
                    window.__brother_har.push({
                        startedDateTime: new Date(this.__har_start).toISOString(),
                        request: { method: this.__har_method, url: this.__har_url },
                        response: { status: this.status, statusText: this.statusText },
                        time: Date.now() - this.__har_start
                    });
                });
                return origXHR.apply(this, arguments);
            };
        }
        return { recording: true };
    })()";

    match page.eval(js).await {
        Ok(_) => {
            let mut guard = state.lock().await;
            guard.har_entries = Some(Vec::new());
            Response::ok_data(ResponseData::Text {
                text: "HAR recording started".to_owned(),
            })
        }
        Err(e) => Response::error(format!("har start failed: {e}")),
    }
}

pub(super) async fn cmd_har_stop(state: &Arc<Mutex<DaemonState>>, path: Option<&str>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Collect captured entries from JS
    let js = r"(() => {
        const entries = window.__brother_har || [];
        delete window.__brother_har;
        return JSON.stringify(entries);
    })()";

    let entries_val = match page.eval(js).await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("har stop failed: {e}")),
    };

    let entries_str = entries_val.as_str().unwrap_or("[]");
    let entries: Vec<serde_json::Value> = serde_json::from_str(entries_str).unwrap_or_default();

    // Build HAR 1.2 format
    let har = serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "brother", "version": env!("CARGO_PKG_VERSION") },
            "entries": entries,
        }
    });

    // Clear recording state
    {
        let mut guard = state.lock().await;
        guard.har_entries = None;
    }

    // Save to file if path provided
    if let Some(p) = path {
        match std::fs::write(p, serde_json::to_string_pretty(&har).unwrap_or_default()) {
            Ok(()) => Response::ok_data(ResponseData::Text {
                text: format!("HAR saved to {p} ({} entries)", entries.len()),
            }),
            Err(e) => Response::error(format!("cannot write HAR file: {e}")),
        }
    } else {
        Response::ok_data(ResponseData::Eval { value: har })
    }
}

pub(super) async fn cmd_screenshot(
    state: &Arc<Mutex<DaemonState>>,
    full_page: bool,
    selector: Option<&str>,
    format: &str,
    quality: u8,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page
        .screenshot(full_page, selector, format, Some(quality))
        .await
    {
        Ok(bytes) => {
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            Response::ok_data(ResponseData::Screenshot { data })
        }
        Err(e) => Response::error(format!("screenshot failed: {e}")),
    }
}

pub(super) async fn cmd_click(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    button: MouseButton,
    click_count: u32,
    delay: u64,
    new_tab: bool,
) -> Response {
    if !new_tab {
        let page = match get_page(state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        return match page.click_with(target, button, click_count, delay).await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly(target).to_string()),
        };
    }
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.key_down("Control").await {
        return Response::error(e.to_string());
    }
    let click_result = page.click(target).await;
    let _ = page.key_up("Control").await;
    if let Err(e) = click_result {
        return Response::error(e.ai_friendly(target).to_string());
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut guard = state.lock().await;
    if let Some(ref browser) = guard.browser {
        if let Ok(pages) = browser.pages().await {
            for p in pages {
                let url = p.url().await.unwrap_or_default();
                if !guard
                    .pages
                    .iter()
                    .any(|ep| futures::executor::block_on(ep.url()).unwrap_or_default() == url)
                {
                    guard.pages.push(p);
                }
            }
        }
        guard.active_tab = guard.pages.len().saturating_sub(1);
    }
    Response::ok()
}

pub(super) async fn cmd_type(
    state: &Arc<Mutex<DaemonState>>,
    target: Option<&str>,
    text: &str,
    delay_ms: u64,
    clear: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if clear && let Some(t) = target {
        return match page.fill(t, text).await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly(t).to_string()),
        };
    }
    match page.type_with_delay(target, text, delay_ms).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(super) async fn cmd_bounding_box(state: &Arc<Mutex<DaemonState>>, target: &str) -> Response {
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
        Err(e) => Response::error(e.ai_friendly(target).to_string()),
    }
}

pub(super) async fn cmd_find(
    state: &Arc<Mutex<DaemonState>>,
    by: &str,
    value: &str,
    name: Option<&str>,
    exact: bool,
    subaction: Option<&str>,
    fill_value: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(sub) = subaction {
        return match page
            .locator_action(by, value, name, exact, sub, fill_value)
            .await
        {
            Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
            Err(e) => Response::error(e.to_string()),
        };
    }
    let result = match by {
        "role" => page.find_by_role(value, name).await,
        "text" => page.find_by_text(value, exact).await,
        "label" => page.find_by_label(value).await,
        "placeholder" => page.find_by_placeholder(value).await,
        "testid" => page.find_by_testid(value).await,
        "alttext" | "alt" => page.find_by_alt_text(value, exact).await,
        "title" => page.find_by_title(value, exact).await,
        _ => {
            return Response::error(format!(
                "unknown locator type '{by}'. Use: role, text, label, placeholder, testid, alttext, title"
            ));
        }
    };
    match result {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(super) async fn cmd_nth(
    state: &Arc<Mutex<DaemonState>>,
    selector: &str,
    index: i64,
    subaction: Option<&str>,
    fill_value: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page
        .nth_action(selector, index, subaction, fill_value)
        .await
    {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(super) async fn cmd_expose(state: &Arc<Mutex<DaemonState>>, name: &str) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "window['{escaped}'] = (...args) => console.log(JSON.stringify({{ fn: '{escaped}', args }}))"
    );
    match page.add_init_script(&js).await {
        Ok(()) => {
            let _ = page.eval(&js).await;
            Response::ok_data(ResponseData::Text {
                text: format!("function '{name}' exposed on window"),
            })
        }
        Err(e) => Response::error(format!("expose: {e}")),
    }
}

pub(super) fn cmd_device_list() -> Response {
    let names = brother::DevicePreset::list_names();
    let descriptions: Vec<serde_json::Value> = names
        .iter()
        .filter_map(|n| {
            brother::DevicePreset::lookup(n).map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "width": p.width,
                    "height": p.height,
                    "user_agent": p.user_agent,
                })
            })
        })
        .collect();
    Response::ok_data(ResponseData::Eval {
        value: serde_json::Value::Array(descriptions),
    })
}

pub(super) async fn cmd_device(state: &Arc<Mutex<DaemonState>>, name: &str) -> Response {
    let Some(preset) = brother::DevicePreset::lookup(name) else {
        let names = brother::DevicePreset::list_names().join(", ");
        return Response::error(format!("unknown device '{name}'. Available: {names}"));
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.set_viewport(preset.width, preset.height).await {
        return Response::error(e.to_string());
    }
    if let Err(e) = page.set_user_agent(preset.user_agent).await {
        return Response::error(e.to_string());
    }
    Response::ok_data(ResponseData::Text {
        text: format!(
            "emulating {} ({}x{}, {})",
            preset.name, preset.width, preset.height, preset.user_agent
        ),
    })
}

pub(super) async fn cmd_extra_headers(
    state: &Arc<Mutex<DaemonState>>,
    headers_json: &str,
) -> Response {
    let map: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(headers_json) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("invalid headers JSON: {e}")),
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.set_extra_headers(map).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(super) async fn cmd_dialog_message(state: &Arc<Mutex<DaemonState>>) -> Response {
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

pub(super) async fn cmd_console(state: &Arc<Mutex<DaemonState>>, clear: bool) -> Response {
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

pub(super) async fn cmd_errors(state: &Arc<Mutex<DaemonState>>, clear: bool) -> Response {
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

pub(super) async fn cmd_screencast_start(
    state: &Arc<Mutex<DaemonState>>,
    format: &str,
    quality: u32,
    max_width: Option<u32>,
    max_height: Option<u32>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::{
        StartScreencastFormat, StartScreencastParams,
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let fmt = if format == "png" {
        StartScreencastFormat::Png
    } else {
        StartScreencastFormat::Jpeg
    };
    let params = StartScreencastParams::builder()
        .format(fmt)
        .quality(i64::from(quality))
        .max_width(i64::from(max_width.unwrap_or(1280)))
        .max_height(i64::from(max_height.unwrap_or(720)))
        .build();
    match page.inner().execute(params).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: format!("screencast started ({format}, quality={quality})"),
        }),
        Err(e) => Response::error(format!("screencast start failed: {e}")),
    }
}

pub(super) async fn cmd_screencast_stop(state: &Arc<Mutex<DaemonState>>) -> Response {
    use chromiumoxide::cdp::browser_protocol::page::StopScreencastParams;
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.inner().execute(StopScreencastParams::default()).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: "screencast stopped".to_owned(),
        }),
        Err(e) => Response::error(format!("screencast stop failed: {e}")),
    }
}

pub(super) async fn cmd_status(state: &Arc<Mutex<DaemonState>>) -> Response {
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
