//! Tab and window management handlers.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use crate::daemon::state::{DaemonState, ensure_browser};

pub(in crate::daemon) async fn cmd_window_new(
    state: &Arc<Mutex<DaemonState>>,
    width: Option<u32>,
    height: Option<u32>,
) -> Response {
    ensure_browser(state).await.ok();
    let mut guard = state.lock().await;
    let Some(ref browser) = guard.browser else {
        return Response::error("no browser running");
    };
    match browser.new_page("about:blank").await {
        Ok(page) => {
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

pub(in crate::daemon) async fn cmd_tab_new(
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

pub(in crate::daemon) async fn cmd_tab_list(state: &Arc<Mutex<DaemonState>>) -> Response {
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

pub(in crate::daemon) async fn cmd_tab_select(
    state: &Arc<Mutex<DaemonState>>,
    index: usize,
) -> Response {
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

pub(in crate::daemon) async fn cmd_tab_close(
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
