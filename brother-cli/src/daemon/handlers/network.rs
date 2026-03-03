//! Network handlers: route, unroute, requests, downloads, response_body.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData, RouteAction};

use crate::daemon::state::{DaemonState, get_page};

/// Add a network interception route.
pub(in crate::daemon) async fn cmd_route(
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
pub(in crate::daemon) async fn cmd_unroute(
    state: &Arc<Mutex<DaemonState>>,
    pattern: &str,
) -> Response {
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
pub(in crate::daemon) async fn cmd_requests(
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
pub(in crate::daemon) async fn cmd_set_download_path(
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
pub(in crate::daemon) async fn cmd_downloads(
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
pub(in crate::daemon) async fn cmd_wait_for_download(
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
pub(in crate::daemon) async fn cmd_download(
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
pub(in crate::daemon) async fn cmd_response_body(
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

/// Start recording HTTP traffic as HAR (HTTP Archive).
pub(in crate::daemon) async fn cmd_har_start(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

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

/// Stop HAR recording and save the archive.
pub(in crate::daemon) async fn cmd_har_stop(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

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

    let har = serde_json::json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "brother", "version": env!("CARGO_PKG_VERSION") },
            "entries": entries,
        }
    });

    {
        let mut guard = state.lock().await;
        guard.har_entries = None;
    }

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
