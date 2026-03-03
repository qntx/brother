//! Navigation handlers: navigate, connect, frame, `main_frame`.

use std::sync::Arc;

use brother::{Browser, Error};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData, WaitStrategy};

use crate::daemon::state::{DaemonState, get_page};

pub(in crate::daemon) async fn cmd_navigate(
    state: &Arc<Mutex<DaemonState>>,
    url: &str,
    wait: WaitStrategy,
    headers: std::collections::HashMap<String, String>,
) -> Response {
    // Domain filter: block navigation to non-allowed domains.
    {
        let guard = state.lock().await;
        if !guard.allowed_domains.is_empty()
            && let Some(host) = crate::domain_filter::extract_host(url)
            && !crate::domain_filter::is_allowed(&host, &guard.allowed_domains)
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
    let has_headers = !headers.is_empty();
    if has_headers {
        let map: serde_json::Map<String, serde_json::Value> = headers
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        if let Err(e) = page.set_extra_headers(map).await {
            return Response::error(format!("failed to set headers: {e}"));
        }
    }
    if let Err(e) = page.goto(url).await {
        if has_headers {
            let _ = page.set_extra_headers(serde_json::Map::new()).await;
        }
        return Response::error(format!("navigation failed: {e}"));
    }
    if matches!(wait, WaitStrategy::NetworkIdle) {
        let _ = page.wait_for_navigation().await;
    }
    if has_headers {
        let _ = page.set_extra_headers(serde_json::Map::new()).await;
    }
    let u = page.url().await.unwrap_or_default();
    let t = page.title().await.unwrap_or_default();
    Response::ok_data(ResponseData::Navigate { url: u, title: t })
}

/// Connect to an existing browser via CDP websocket URL or debugging port.
pub(in crate::daemon) async fn cmd_connect(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
) -> Response {
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
pub(in crate::daemon) async fn cmd_frame(
    state: &Arc<Mutex<DaemonState>>,
    selector: &str,
) -> Response {
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
pub(in crate::daemon) async fn cmd_main_frame(state: &Arc<Mutex<DaemonState>>) -> Response {
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
