//! Daemon shared state and browser lifecycle management.

use std::sync::Arc;

use brother::{Browser, BrowserConfig, Page};
use futures::StreamExt;
use tokio::sync::Mutex;

use crate::protocol::Response;

/// Shared state across connections.
pub(crate) struct DaemonState {
    /// Session name for port/pid file management.
    pub session: String,
    pub browser: Option<Browser>,
    /// All open tabs (pages). Index 0 is the first tab opened.
    pub pages: Vec<Page>,
    /// Index into `pages` for the currently active tab.
    pub active_tab: usize,
    /// Currently active frame (None = main frame).
    pub active_frame_id: Option<String>,
    /// Active network interception patterns.
    pub routes: Vec<String>,
    /// Captured network requests (from JS interception).
    pub captured_requests: Vec<serde_json::Value>,
    /// Download directory path.
    pub download_path: Option<String>,
    /// Pending launch configuration (set by `Launch` request before browser starts).
    pub launch_config: Option<BrowserConfig>,
    pub last_activity: tokio::time::Instant,
    /// Allowed domain patterns for navigation security filter.
    pub allowed_domains: Vec<String>,
    /// Pending color scheme to apply after browser launch.
    pub pending_color_scheme: Option<String>,
    /// Pending storage state file to load after browser launch.
    pub pending_storage_state: Option<String>,
    /// Action policy cache (hot-reloaded from file).
    pub policy_cache: Option<crate::policy::PolicyCache>,
    /// Pending confirmation queue for policy confirm decisions.
    pub confirmations: crate::policy::ConfirmationQueue,
    /// HAR recording: captured entries while recording is active.
    pub har_entries: Option<Vec<serde_json::Value>>,
}

pub(crate) async fn ensure_browser(state: &Arc<Mutex<DaemonState>>) -> Result<(), Response> {
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
pub(crate) async fn get_page(state: &Arc<Mutex<DaemonState>>) -> Result<Page, Response> {
    ensure_browser(state).await?;
    let guard = state.lock().await;
    guard
        .pages
        .get(guard.active_tab)
        .cloned()
        .ok_or_else(|| Response::error("no active page"))
}
