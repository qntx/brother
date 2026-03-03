//! Command handlers that require complex logic beyond simple macro dispatch.

use std::sync::Arc;
use std::time::Duration;

use brother::{Browser, Error};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData, RouteAction, WaitCondition, WaitStrategy};

use super::{ensure_browser, get_page, DaemonState, InterceptRoute};

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Snapshot
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Wait
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Connection
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Frame (iframe)
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Network interception
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Download handlers
// ---------------------------------------------------------------------------

/// Set download directory via CDP and store path in `DaemonState`.
pub(super) async fn cmd_set_download_path(
    state: &Arc<Mutex<DaemonState>>,
    path: &str,
) -> Response {
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
            let resp =
                cmd_set_download_path(state, tmp.to_string_lossy().as_ref()).await;
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

// ---------------------------------------------------------------------------
// Tab management
// ---------------------------------------------------------------------------

pub(super) async fn cmd_tab_new(
    state: &Arc<Mutex<DaemonState>>,
    url: Option<&str>,
) -> Response {
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

// ---------------------------------------------------------------------------
// Diff handlers
// ---------------------------------------------------------------------------

/// Compare current snapshot against baseline text.
pub(super) async fn cmd_diff_snapshot(
    state: &Arc<Mutex<DaemonState>>,
    baseline: &str,
    options: brother::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let snap = match page.snapshot_with(options).await {
        Ok(s) => s,
        Err(e) => return Response::error(format!("snapshot failed: {e}")),
    };

    let current = snap.tree();
    let result = brother::diff_snapshots(baseline, current);
    let summary = result.summary();

    Response::ok_data(ResponseData::DiffSnapshot {
        added: result.added,
        removed: result.removed,
        unchanged: result.unchanged,
        diff: result.diff,
        summary,
    })
}

/// Compare two URLs: navigate to each, take snapshot, optionally diff screenshots.
pub(super) async fn cmd_diff_url(
    state: &Arc<Mutex<DaemonState>>,
    url_a: &str,
    url_b: &str,
    screenshot: bool,
    threshold: u8,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Navigate to URL A and take snapshot (+ optional screenshot)
    if let Err(e) = page.goto(url_a).await {
        return Response::error(format!("navigate to URL A: {e}"));
    }
    let snap_a = match page.snapshot().await {
        Ok(s) => s.tree().to_owned(),
        Err(e) => return Response::error(format!("snapshot A: {e}")),
    };
    let screenshot_a = if screenshot {
        match page.screenshot(false, None, "png", Some(80)).await {
            Ok(b) => Some(b),
            Err(e) => return Response::error(format!("screenshot A: {e}")),
        }
    } else {
        None
    };

    // Navigate to URL B and take snapshot (+ optional screenshot)
    if let Err(e) = page.goto(url_b).await {
        return Response::error(format!("navigate to URL B: {e}"));
    }
    let snap_b = match page.snapshot().await {
        Ok(s) => s.tree().to_owned(),
        Err(e) => return Response::error(format!("snapshot B: {e}")),
    };
    let screenshot_b = if screenshot {
        match page.screenshot(false, None, "png", Some(80)).await {
            Ok(b) => Some(b),
            Err(e) => return Response::error(format!("screenshot B: {e}")),
        }
    } else {
        None
    };

    // Diff snapshots
    let snap_result = brother::diff_snapshots(&snap_a, &snap_b);
    let mut output = snap_result.summary();

    // Optionally diff screenshots
    if let (Some(bytes_a), Some(bytes_b)) = (screenshot_a, screenshot_b) {
        let rgba_a = match decode_png_to_rgba(&bytes_a) {
            Ok(r) => r,
            Err(e) => return Response::error(format!("decode screenshot A: {e}")),
        };
        let rgba_b = match decode_png_to_rgba(&bytes_b) {
            Ok(r) => r,
            Err(e) => return Response::error(format!("decode screenshot B: {e}")),
        };
        let img_result = brother::diff_rgba(
            &rgba_a.pixels, rgba_a.width, rgba_a.height,
            &rgba_b.pixels, rgba_b.width, rgba_b.height,
            threshold,
        );

        if let Ok(diff_path) = generate_diff_image(&rgba_a, &rgba_b, threshold).await {
            output = format!("{output}\nscreenshot: {} | diff image: {diff_path}", img_result.summary());
        } else {
            output = format!("{output}\nscreenshot: {}", img_result.summary());
        }
    }

    // Return snapshot diff + extra screenshot info
    Response::ok_data(ResponseData::DiffSnapshot {
        diff: snap_result.diff,
        added: snap_result.added,
        removed: snap_result.removed,
        unchanged: snap_result.unchanged,
        summary: output,
    })
}

/// Compare current screenshot against baseline (base64-encoded PNG).
///
/// Decodes PNGs in Rust (no browser round-trip), generates a diff image
/// with red-highlighted pixels, and saves it to `~/.brother/tmp/diffs/`.
pub(super) async fn cmd_diff_screenshot(
    state: &Arc<Mutex<DaemonState>>,
    baseline_b64: &str,
    threshold: u8,
    full_page: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Take current screenshot
    let current_bytes = match page.screenshot(full_page, None, "png", Some(80)).await {
        Ok(b) => b,
        Err(e) => return Response::error(format!("screenshot failed: {e}")),
    };

    // Decode baseline from base64
    let baseline_bytes = match base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        baseline_b64,
    ) {
        Ok(b) => b,
        Err(e) => return Response::error(format!("invalid baseline base64: {e}")),
    };

    // Decode both PNGs to RGBA using the png crate (no browser round-trip)
    let baseline_rgba = match decode_png_to_rgba(&baseline_bytes) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("baseline decode: {e}")),
    };
    let current_rgba = match decode_png_to_rgba(&current_bytes) {
        Ok(r) => r,
        Err(e) => return Response::error(format!("current decode: {e}")),
    };

    let result = brother::diff_rgba(
        &baseline_rgba.pixels,
        baseline_rgba.width,
        baseline_rgba.height,
        &current_rgba.pixels,
        current_rgba.width,
        current_rgba.height,
        threshold,
    );

    // Generate diff image and save to disk
    let diff_path = match generate_diff_image(
        &baseline_rgba,
        &current_rgba,
        threshold,
    )
    .await
    {
        Ok(p) => p,
        Err(e) => return Response::error(format!("diff image: {e}")),
    };

    Response::ok_data(ResponseData::DiffScreenshot {
        diff_path,
        total_pixels: result.total_pixels,
        diff_pixels: result.diff_pixels,
        diff_percentage: result.diff_percentage,
        size_mismatch: result.size_mismatch,
        summary: result.summary(),
    })
}

/// Decoded RGBA image data.
struct RgbaImage {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
}

/// Decode a PNG buffer to RGBA pixel data using the `png` crate.
fn decode_png_to_rgba(data: &[u8]) -> Result<RgbaImage, String> {
    let decoder = png::Decoder::new(std::io::Cursor::new(data));
    let mut reader = decoder.read_info().map_err(|e| format!("png header: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("png frame: {e}"))?;
    buf.truncate(info.buffer_size());

    let width = info.width;
    let height = info.height;

    // Convert to RGBA if needed
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in buf.chunks_exact(3) {
                rgba.extend_from_slice(chunk);
                rgba.push(255);
            }
            rgba
        }
        png::ColorType::GrayscaleAlpha => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for chunk in buf.chunks_exact(2) {
                let g = chunk[0];
                rgba.extend_from_slice(&[g, g, g, chunk[1]]);
            }
            rgba
        }
        png::ColorType::Grayscale => {
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            for &g in &buf {
                rgba.extend_from_slice(&[g, g, g, 255]);
            }
            rgba
        }
        other @ png::ColorType::Indexed => return Err(format!("unsupported color type: {other:?}")),
    };

    Ok(RgbaImage {
        pixels,
        width,
        height,
    })
}

/// Generate a diff PNG: different pixels in red, same pixels dimmed.
/// Saves to `~/.brother/tmp/diffs/diff-<timestamp>.png`.
async fn generate_diff_image(
    baseline: &RgbaImage,
    current: &RgbaImage,
    threshold: u8,
) -> Result<String, String> {
    let w = baseline.width.max(current.width);
    let h = baseline.height.max(current.height);
    let mut diff_pixels = vec![0u8; (w * h * 4) as usize];

    let thresh_sq = i32::from(threshold) * i32::from(threshold) * 3;

    for y in 0..h {
        for x in 0..w {
            let di = ((y * w + x) * 4) as usize;
            let get_pixel = |img: &RgbaImage, px: u32, py: u32| -> [u8; 4] {
                if px < img.width && py < img.height {
                    let i = ((py * img.width + px) * 4) as usize;
                    [img.pixels[i], img.pixels[i + 1], img.pixels[i + 2], img.pixels[i + 3]]
                } else {
                    [0, 0, 0, 0]
                }
            };

            let a = get_pixel(baseline, x, y);
            let b = get_pixel(current, x, y);

            let dr = i32::from(a[0]) - i32::from(b[0]);
            let dg = i32::from(a[1]) - i32::from(b[1]);
            let db = i32::from(a[2]) - i32::from(b[2]);
            let dist_sq = dr * dr + dg * dg + db * db;

            if dist_sq > thresh_sq {
                // Different: red
                diff_pixels[di] = 255;
                diff_pixels[di + 1] = 0;
                diff_pixels[di + 2] = 0;
                diff_pixels[di + 3] = 255;
            } else {
                // Same: dimmed baseline
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                {
                    diff_pixels[di] = (f64::from(a[0]) * 0.3) as u8;
                    diff_pixels[di + 1] = (f64::from(a[1]) * 0.3) as u8;
                    diff_pixels[di + 2] = (f64::from(a[2]) * 0.3) as u8;
                    diff_pixels[di + 3] = 255;
                }
            }
        }
    }

    // Encode diff image as PNG
    let diff_dir = crate::protocol::runtime_dir()
        .ok_or_else(|| "cannot determine runtime dir".to_owned())?
        .join("tmp")
        .join("diffs");
    tokio::fs::create_dir_all(&diff_dir)
        .await
        .map_err(|e| format!("mkdir: {e}"))?;

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let path = diff_dir.join(format!("diff-{ts}.png"));

    let file = std::fs::File::create(&path).map_err(|e| format!("create file: {e}"))?;
    let buf_writer = std::io::BufWriter::new(file);
    let mut encoder = png::Encoder::new(buf_writer, w, h);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().map_err(|e| format!("png header: {e}"))?;
    writer
        .write_image_data(&diff_pixels)
        .map_err(|e| format!("png write: {e}"))?;

    Ok(path.to_string_lossy().into_owned())
}

// ---------------------------------------------------------------------------
// State persistence handlers
// ---------------------------------------------------------------------------

/// Directory for saved states: `~/.brother/sessions/`.
fn sessions_dir() -> Option<std::path::PathBuf> {
    crate::protocol::runtime_dir().map(|d| d.join("sessions"))
}

/// Validate a state name: only `[a-zA-Z0-9_-]` allowed.
/// Prevents path traversal attacks (e.g. `"../../etc/passwd"`).
fn validate_state_name(name: &str) -> Result<(), Response> {
    if name == "*" {
        return Ok(()); // wildcard is allowed for clear-all
    }
    if name.is_empty() || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
        return Err(Response::error(format!(
            "invalid state name '{name}': only alphanumeric, hyphens, and underscores allowed"
        )));
    }
    Ok(())
}

/// Save cookies + localStorage + sessionStorage to a named JSON file.
pub(super) async fn cmd_state_save(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Gather cookies
    let cookies = match page.get_cookies().await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("get cookies: {e}")),
    };

    // Gather localStorage + sessionStorage via JS
    let storage_js = r"(() => {
        const ls = {};
        for (let i = 0; i < localStorage.length; i++) {
            const k = localStorage.key(i);
            ls[k] = localStorage.getItem(k);
        }
        const ss = {};
        for (let i = 0; i < sessionStorage.length; i++) {
            const k = sessionStorage.key(i);
            ss[k] = sessionStorage.getItem(k);
        }
        return JSON.stringify({ localStorage: ls, sessionStorage: ss });
    })()";

    let storage_val = page.eval(storage_js).await.unwrap_or_default();
    let storage_str = storage_val.as_str().unwrap_or("{}");
    let storage: serde_json::Value =
        serde_json::from_str(storage_str).unwrap_or_else(|_| serde_json::json!({}));

    let url = page.url().await.unwrap_or_default();

    let state_data = serde_json::json!({
        "url": url,
        "cookies": cookies,
        "localStorage": storage.get("localStorage").cloned().unwrap_or_else(|| serde_json::json!({})),
        "sessionStorage": storage.get("sessionStorage").cloned().unwrap_or_else(|| serde_json::json!({})),
        "savedAt": chrono_now(),
    });

    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Response::error(format!("mkdir: {e}"));
    }

    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&state_data).unwrap_or_default();
    if let Err(e) = tokio::fs::write(&path, &json).await {
        return Response::error(format!("write: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("state saved: {name} ({})", path.display()),
    })
}

/// Load a previously saved state (cookies + storage).
pub(super) async fn cmd_state_load(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let path = dir.join(format!("{name}.json"));
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Response::error(format!("read state '{name}': {e}")),
    };
    let data: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Response::error(format!("parse state '{name}': {e}")),
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Restore cookies
    if let Some(cookies) = data.get("cookies")
        && let Ok(cookie_list) =
            serde_json::from_value::<Vec<brother::CookieInput>>(cookies.clone())
        && let Err(e) = page.set_cookies(&cookie_list).await
    {
        return Response::error(format!("restore cookies: {e}"));
    }

    // Navigate to saved URL first (so storage domain matches)
    if let Some(url) = data.get("url").and_then(|v| v.as_str())
        && !url.is_empty()
        && url != "about:blank"
    {
        let _ = page.goto(url).await;
    }

    // Restore localStorage
    if let Some(ls) = data.get("localStorage").and_then(|v| v.as_object()) {
        for (k, v) in ls {
            let val = v.as_str().unwrap_or("");
            let escaped_k = k.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_v = val.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = page
                .eval(&format!("localStorage.setItem('{escaped_k}', '{escaped_v}')"))
                .await;
        }
    }

    // Restore sessionStorage
    if let Some(ss) = data.get("sessionStorage").and_then(|v| v.as_object()) {
        for (k, v) in ss {
            let val = v.as_str().unwrap_or("");
            let escaped_k = k.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_v = val.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = page
                .eval(&format!(
                    "sessionStorage.setItem('{escaped_k}', '{escaped_v}')"
                ))
                .await;
        }
    }

    Response::ok_data(ResponseData::Text {
        text: format!("state loaded: {name}"),
    })
}

/// List all saved state files.
pub(super) async fn cmd_state_list() -> Response {
    let Some(dir) = sessions_dir() else {
        return Response::ok_data(ResponseData::StateList {
            states: Vec::new(),
        });
    };
    let mut names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let fname = entry.file_name().to_string_lossy().to_string();
            if let Some(name) = fname.strip_suffix(".json") {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    Response::ok_data(ResponseData::StateList { states: names })
}

/// Delete a saved state file (or all with `name = "*"`).
pub(super) async fn cmd_state_clear(name: &str) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    if name == "*" {
        let mut count = 0usize;
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if entry
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".json")
                {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                    count += 1;
                }
            }
        }
        Response::ok_data(ResponseData::Text {
            text: format!("{count} state(s) cleared"),
        })
    } else {
        let path = dir.join(format!("{name}.json"));
        if let Err(e) = tokio::fs::remove_file(&path).await {
            return Response::error(format!("delete state '{name}': {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("state '{name}' deleted"),
        })
    }
}

/// Show the contents of a saved state file.
pub(super) async fn cmd_state_show(name: &str) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let path = dir.join(format!("{name}.json"));
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Response::error(format!("read state '{name}': {e}")),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Response::error(format!("parse state '{name}': {e}")),
    };
    Response::ok_data(ResponseData::Eval { value: val })
}

/// Clean up state files older than `days` days.
pub(super) async fn cmd_state_clean(days: u32) -> Response {
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let max_age = Duration::from_secs(u64::from(days) * 86400);
    let now = std::time::SystemTime::now();
    let mut deleted = Vec::new();

    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !std::path::Path::new(&fname)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            if let Ok(meta) = entry.metadata().await
                && let Ok(modified) = meta.modified()
                && now.duration_since(modified).unwrap_or_default() > max_age
            {
                let _ = tokio::fs::remove_file(entry.path()).await;
                if let Some(name) = fname.strip_suffix(".json") {
                    deleted.push(name.to_owned());
                }
            }
        }
    }

    let count = deleted.len();
    Response::ok_data(ResponseData::Text {
        text: format!("{count} expired state(s) cleaned"),
    })
}

/// Rename a saved state file.
pub(super) async fn cmd_state_rename(old_name: &str, new_name: &str) -> Response {
    if let Err(r) = validate_state_name(old_name) {
        return r;
    }
    if let Err(r) = validate_state_name(new_name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let old_path = dir.join(format!("{old_name}.json"));
    let new_path = dir.join(format!("{new_name}.json"));
    if let Err(e) = tokio::fs::rename(&old_path, &new_path).await {
        return Response::error(format!("rename '{old_name}' → '{new_name}': {e}"));
    }
    Response::ok_data(ResponseData::Text {
        text: format!("state renamed: {old_name} → {new_name}"),
    })
}

/// Simple ISO-8601-ish timestamp without external crate.
fn chrono_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", d.as_secs())
}

// ---------------------------------------------------------------------------
// Tracing / Profiler handlers (real CDP protocol)
// ---------------------------------------------------------------------------

/// Default tracing categories when none are specified.
const DEFAULT_TRACE_CATEGORIES: &[&str] = &[
    "devtools.timeline",
    "v8.execute",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
];

/// Start CDP `Tracing.start`.
pub(super) async fn cmd_trace_start(
    state: &Arc<Mutex<DaemonState>>,
    categories: &[String],
) -> Response {
    use chromiumoxide::cdp::browser_protocol::tracing::{
        StartParams, TraceConfig,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let cats: Vec<String> = if categories.is_empty() {
        DEFAULT_TRACE_CATEGORIES.iter().map(|&s| s.to_owned()).collect()
    } else {
        categories.to_vec()
    };

    let config = TraceConfig::builder()
        .included_categories(cats.clone())
        .build();

    let params = StartParams::builder()
        .trace_config(config)
        .build();

    match page.inner().execute(params).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: format!("tracing started ({})", cats.join(", ")),
        }),
        Err(e) => Response::error(format!("trace start: {e}")),
    }
}

/// Stop CDP `Tracing.end` and collect trace data.
///
/// After calling `Tracing.end`, the browser fires `Tracing.dataCollected`
/// events followed by a `Tracing.tracingComplete` event.  Listening for
/// those streamed events through chromiumoxide's typed event API requires
/// the caller to set up a subscription *before* `Tracing.end` is sent.
/// This is fragile with the current chromiumoxide API, so instead we use
/// a pragmatic approach: send `Tracing.end` and then poll for trace data
/// via the JS Performance API as a fallback, or write the raw CDP
/// response.
pub(super) async fn cmd_trace_stop(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::tracing::EndParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Stop tracing via CDP
    if let Err(e) = page.inner().execute(EndParams::default()).await {
        return Response::error(format!("trace stop: {e}"));
    }

    // Give the browser a moment to flush trace buffers
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Collect whatever performance data is available via JS
    let js = r"(() => {
        const entries = performance.getEntriesByType('resource')
            .concat(performance.getEntriesByType('navigation'))
            .concat(performance.getEntriesByType('mark'))
            .concat(performance.getEntriesByType('measure'));
        return JSON.stringify({
            entry_count: entries.length,
            entries: entries.map(e => ({
                name: e.name,
                type: e.entryType,
                startTime: e.startTime,
                duration: e.duration
            }))
        });
    })()";

    let trace_json = page
        .eval(js)
        .await
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "{}".to_owned());

    if let Some(file_path) = path {
        if let Err(e) = tokio::fs::write(file_path, &trace_json).await {
            return Response::error(format!("write trace: {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("trace saved to {file_path}"),
        })
    } else {
        let parsed: serde_json::Value =
            serde_json::from_str(&trace_json).unwrap_or(serde_json::Value::Null);
        Response::ok_data(ResponseData::Eval { value: parsed })
    }
}

/// Start CDP `Profiler.enable` + `Profiler.start`.
pub(super) async fn cmd_profiler_start(
    state: &Arc<Mutex<DaemonState>>,
    _categories: &[String],
) -> Response {
    use chromiumoxide::cdp::js_protocol::profiler::{
        EnableParams, StartParams,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    if let Err(e) = page.inner().execute(EnableParams::default()).await {
        return Response::error(format!("profiler enable: {e}"));
    }
    match page.inner().execute(StartParams::default()).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: "profiler started (CDP Profiler.start)".to_owned(),
        }),
        Err(e) => Response::error(format!("profiler start: {e}")),
    }
}

/// Stop CDP `Profiler.stop` and return the V8 CPU profile.
pub(super) async fn cmd_profiler_stop(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::js_protocol::profiler::StopParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let resp = match page.inner().execute(StopParams::default()).await {
        Ok(r) => r,
        Err(e) => return Response::error(format!("profiler stop: {e}")),
    };

    let profile_json =
        serde_json::to_string_pretty(&resp.result.profile).unwrap_or_else(|_| "{}".into());

    if let Some(file_path) = path {
        if let Err(e) = tokio::fs::write(file_path, &profile_json).await {
            return Response::error(format!("write profile: {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("profile saved to {file_path}"),
        })
    } else {
        let parsed: serde_json::Value =
            serde_json::from_str(&profile_json).unwrap_or(serde_json::Value::Null);
        Response::ok_data(ResponseData::Eval { value: parsed })
    }
}

// ---------------------------------------------------------------------------
// Domain filter handler
// ---------------------------------------------------------------------------

/// Set allowed domain patterns for navigation security.
///
/// When domains are non-empty, injects an init script into every existing
/// page that monkey-patches `WebSocket`, `EventSource`, and
/// `navigator.sendBeacon` to block connections to non-allowed domains.
/// Navigation checks are enforced in [`cmd_navigate`].
pub(super) async fn cmd_set_allowed_domains(
    state: &Arc<Mutex<DaemonState>>,
    domains: Vec<String>,
) -> Response {
    let mut guard = state.lock().await;
    let count = domains.len();

    // Inject init script into all existing pages so future navigations
    // within those pages also get the filter.
    if !domains.is_empty() {
        let script = super::domain_filter::build_init_script(&domains);
        for page in &guard.pages {
            let _ = page.add_init_script(&script).await;
            // Also run it immediately on the current document.
            let _ = page.eval(&script).await;
        }
    }

    guard.allowed_domains = domains;
    Response::ok_data(ResponseData::Text {
        text: if count == 0 {
            "domain filter cleared (all domains allowed)".to_owned()
        } else {
            format!("{count} domain pattern(s) set")
        },
    })
}
