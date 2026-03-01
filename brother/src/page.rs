//! High-level page abstraction for browser interaction.

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, FocusParams, ResolveNodeParams};
use chromiumoxide::cdp::browser_protocol::input::{DispatchKeyEventParams, DispatchKeyEventType};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, GetNavigationHistoryParams, NavigateToHistoryEntryParams,
};
use chromiumoxide::cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams};
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::snapshot::{self, Ref, RefMap, Snapshot, SnapshotOptions};

/// A browser page with ref-based interaction support.
///
/// Wraps a `chromiumoxide::Page` and adds accessibility snapshot / ref
/// tracking on top.
#[derive(Debug, Clone)]
pub struct Page {
    inner: chromiumoxide::Page,
    /// Cached refs from the most recent snapshot.
    refs: Arc<Mutex<RefMap>>,
}

impl Page {
    /// Wrap a chromiumoxide page.
    pub(crate) fn new(inner: chromiumoxide::Page) -> Self {
        Self {
            inner,
            refs: Arc::new(Mutex::new(RefMap::new())),
        }
    }

    // -----------------------------------------------------------------------
    // Navigation
    // -----------------------------------------------------------------------

    /// Navigate to a URL and wait for the page to load.
    ///
    /// # Errors
    ///
    /// Returns an error if navigation fails or the URL is invalid.
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.inner.goto(url).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go back in history.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn go_back(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        #[allow(clippy::cast_possible_wrap)]
        let entry_id = idx.saturating_sub(1) as i64;
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go forward in history.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn go_forward(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        #[allow(clippy::cast_possible_wrap)]
        let entry_id = (idx + 1) as i64;
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Reload the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn reload(&self) -> Result<()> {
        self.inner.reload().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Wait for navigation to complete (network idle heuristic).
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_navigation(&self) -> Result<()> {
        self.inner.wait_for_navigation().await.map_err(Error::Cdp)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Snapshot
    // -----------------------------------------------------------------------

    /// Capture an accessibility snapshot with default options.
    ///
    /// This is the primary method for AI agents to observe page state.
    /// Returns a [`Snapshot`] containing the formatted tree and ref map.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP accessibility call fails.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        self.snapshot_with(SnapshotOptions::default()).await
    }

    /// Capture an accessibility snapshot with custom options.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP accessibility call fails.
    pub async fn snapshot_with(&self, options: SnapshotOptions) -> Result<Snapshot> {
        let result = self
            .inner
            .execute(GetFullAxTreeParams::default())
            .await
            .map_err(Error::Cdp)?;

        let nodes: Vec<serde_json::Value> = serde_json::to_value(&result.result.nodes)
            .and_then(serde_json::from_value)
            .map_err(|e| Error::Snapshot(format!("failed to parse AX tree: {e}")))?;

        let snap = snapshot::build_snapshot(&nodes, &options);

        // Cache refs for subsequent ref-based interactions
        *self.refs.lock().await = snap.refs().clone();

        Ok(snap)
    }

    // -----------------------------------------------------------------------
    // Ref-based interaction
    // -----------------------------------------------------------------------

    /// Click an element by ref (e.g. `"e1"` or `"@e1"`).
    ///
    /// The ref must come from a prior [`snapshot`](Self::snapshot) call.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn click_ref(&self, ref_id: &str) -> Result<()> {
        let r = self.resolve_ref(ref_id).await?;
        let oid = self.resolve_ref_to_object(&r).await?;
        let center = self.get_center_from_object(oid).await?;
        self.inner.click(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Fill (clear + type) an element by ref.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn fill_ref(&self, ref_id: &str, text: &str) -> Result<()> {
        let r = self.resolve_ref(ref_id).await?;
        self.focus_ref_element(&r).await?;
        // Select all existing content and delete
        self.key_press("Control+a").await?;
        self.key_press("Delete").await?;
        self.type_text(text).await
    }

    /// Type text into a focused element by ref (appends, does not clear).
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn type_ref(&self, ref_id: &str, text: &str) -> Result<()> {
        let r = self.resolve_ref(ref_id).await?;
        self.focus_ref_element(&r).await?;
        self.type_text(text).await
    }

    /// Focus an element by ref.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn focus_ref(&self, ref_id: &str) -> Result<()> {
        let r = self.resolve_ref(ref_id).await?;
        self.focus_ref_element(&r).await
    }

    /// Hover an element by ref.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn hover_ref(&self, ref_id: &str) -> Result<()> {
        let r = self.resolve_ref(ref_id).await?;
        let oid = self.resolve_ref_to_object(&r).await?;
        let center = self.get_center_from_object(oid).await?;
        self.inner.move_mouse(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Get the text content of an element by ref.
    ///
    /// # Errors
    ///
    /// Returns [`Error::ElementNotFound`] if the ref is unknown.
    pub async fn text_ref(&self, ref_id: &str) -> Result<String> {
        let r = self.resolve_ref(ref_id).await?;
        let object_id = self.resolve_ref_to_object(&r).await?;

        let result = self
            .inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(object_id)
                    .function_declaration(
                        "function() { return this.innerText || this.textContent || ''; }",
                    )
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;

        let text: String = result.into_value().unwrap_or_default();
        Ok(text)
    }

    // -----------------------------------------------------------------------
    // CSS selector interaction (fallback)
    // -----------------------------------------------------------------------

    /// Click an element by CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn click_selector(&self, selector: &str) -> Result<()> {
        let el = self.find_element(selector).await?;
        el.click().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Fill an element by CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn fill_selector(&self, selector: &str, text: &str) -> Result<()> {
        let el = self.find_element(selector).await?;
        el.click().await.map_err(Error::Cdp)?;
        self.key_press("Control+a").await?;
        self.key_press("Delete").await?;
        self.type_text(text).await
    }

    // -----------------------------------------------------------------------
    // Page info
    // -----------------------------------------------------------------------

    /// Get the current page URL.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn url(&self) -> Result<String> {
        Ok(self
            .inner
            .url()
            .await
            .map_err(Error::Cdp)?
            .unwrap_or_default())
    }

    /// Get the page title.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn title(&self) -> Result<String> {
        let result = self
            .inner
            .evaluate("document.title")
            .await
            .map_err(Error::Cdp)?;
        let title: String = result.into_value().unwrap_or_default();
        Ok(title)
    }

    /// Get the page HTML content.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(Error::Cdp)
    }

    // -----------------------------------------------------------------------
    // Screenshot
    // -----------------------------------------------------------------------

    /// Capture a PNG screenshot of the viewport.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP screenshot command fails.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>> {
        self.inner
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await
            .map_err(Error::Cdp)
    }

    /// Capture a JPEG screenshot of the viewport.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP screenshot command fails.
    pub async fn screenshot_jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        self.inner
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Jpeg)
                    .quality(i64::from(quality))
                    .build(),
            )
            .await
            .map_err(Error::Cdp)
    }

    // -----------------------------------------------------------------------
    // JavaScript evaluation
    // -----------------------------------------------------------------------

    /// Evaluate a `JavaScript` expression and return the raw result.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;

        Ok(result
            .into_value::<serde_json::Value>()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Evaluate JS and deserialize the result into type `T`.
    ///
    /// # Errors
    ///
    /// Returns an error if evaluation or deserialization fails.
    pub async fn eval_as<T: serde::de::DeserializeOwned>(&self, expression: &str) -> Result<T> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;

        result
            .into_value()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e.to_string())))
    }

    // -----------------------------------------------------------------------
    // Keyboard
    // -----------------------------------------------------------------------

    /// Press a single key (e.g. `"Enter"`, `"Tab"`, `"Escape"`, `"Control+a"`).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn key_press(&self, key: &str) -> Result<()> {
        self.inner
            .execute(
                DispatchKeyEventParams::builder()
                    .r#type(DispatchKeyEventType::KeyDown)
                    .key(key)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;

        self.inner
            .execute(
                DispatchKeyEventParams::builder()
                    .r#type(DispatchKeyEventType::KeyUp)
                    .key(key)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;

        Ok(())
    }

    /// Type text character by character with realistic delays.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn type_text(&self, text: &str) -> Result<()> {
        for ch in text.chars() {
            self.inner
                .execute(
                    DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::Char)
                        .text(ch.to_string())
                        .build()
                        .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
                )
                .await
                .map_err(Error::Cdp)?;

            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Wait
    // -----------------------------------------------------------------------

    /// Wait for a CSS selector to appear in the DOM via JS polling.
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let found: bool = self
                .inner
                .evaluate(format!("!!document.querySelector('{selector}')"))
                .await
                .map_err(Error::Cdp)?
                .into_value()
                .unwrap_or(false);

            if found {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "selector \"{selector}\" not found within {timeout:?}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait for a fixed duration.
    pub async fn wait(&self, duration: Duration) {
        tokio::time::sleep(duration).await;
    }

    // -----------------------------------------------------------------------
    // Low-level access
    // -----------------------------------------------------------------------

    /// Access the underlying `chromiumoxide::Page` for advanced CDP operations.
    #[must_use]
    pub const fn inner(&self) -> &chromiumoxide::Page {
        &self.inner
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    /// Resolve a ref id to the cached [`Ref`] metadata.
    async fn resolve_ref(&self, ref_id: &str) -> Result<Ref> {
        let id = ref_id.strip_prefix('@').unwrap_or(ref_id);
        // Also strip "ref=" prefix
        let id = id.strip_prefix("ref=").unwrap_or(id);

        self.refs.lock().await.get(id).cloned().ok_or_else(|| {
            Error::ElementNotFound(format!("ref {id} not found — call snapshot() first"))
        })
    }

    /// Resolve a ref to a `RemoteObjectId` with two-phase strategy:
    /// 1. Fast path: use `backend_node_id` via CDP `DOM.resolveNode`
    /// 2. Fallback: re-locate via JS using ARIA role + name + nth index
    ///
    /// This makes refs survive DOM mutations as long as the element is
    /// still present with the same role and accessible name.
    async fn resolve_ref_to_object(
        &self,
        r: &Ref,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        // Fast path: try backend_node_id
        if r.backend_node_id != 0 {
            if let Ok(oid) = self.resolve_to_object(r.backend_node_id).await {
                return Ok(oid);
            }
            tracing::debug!(
                role = %r.role, name = %r.name,
                "backend_node_id stale, falling back to role+name resolution"
            );
        }

        // Fallback: resolve via JS using role + accessible name
        self.resolve_by_role_name(&r.role, &r.name, r.nth).await
    }

    /// Locate an element by ARIA role + accessible name via `JavaScript`,
    /// returning its `RemoteObjectId`.
    async fn resolve_by_role_name(
        &self,
        role: &str,
        name: &str,
        nth: Option<usize>,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let nth_idx = nth.unwrap_or(0);
        let escaped_name = name.replace('\\', "\\\\").replace('\'', "\\'");
        let escaped_role = role.replace('\\', "\\\\").replace('\'', "\\'");

        // Use TreeWalker over the accessibility tree via JS
        // This queries all elements and filters by computed role + accessible name
        let js = format!(
            r#"(() => {{
                const role = '{escaped_role}';
                const name = '{escaped_name}';
                const nthIdx = {nth_idx};

                // Map common ARIA roles to likely HTML elements/selectors
                const roleSelectors = {{
                    'button': 'button, [role="button"], input[type="button"], input[type="submit"]',
                    'link': 'a[href], [role="link"]',
                    'textbox': 'input:not([type]), input[type="text"], input[type="email"], input[type="password"], input[type="search"], input[type="url"], input[type="tel"], input[type="number"], textarea, [role="textbox"], [contenteditable="true"]',
                    'checkbox': 'input[type="checkbox"], [role="checkbox"]',
                    'radio': 'input[type="radio"], [role="radio"]',
                    'combobox': 'select, [role="combobox"]',
                    'heading': 'h1, h2, h3, h4, h5, h6, [role="heading"]',
                    'listbox': 'select[multiple], [role="listbox"]',
                    'menuitem': '[role="menuitem"]',
                    'option': 'option, [role="option"]',
                    'slider': 'input[type="range"], [role="slider"]',
                    'switch': '[role="switch"]',
                    'tab': '[role="tab"]',
                    'searchbox': 'input[type="search"], [role="searchbox"]',
                    'spinbutton': 'input[type="number"], [role="spinbutton"]',
                }};

                const selector = roleSelectors[role] || `[${{`role="${{role}}"`}}]`;
                const candidates = document.querySelectorAll(selector);
                let matches = [];

                for (const el of candidates) {{
                    // Check accessible name (innerText, aria-label, title, etc.)
                    const accName = el.getAttribute('aria-label')
                        || el.getAttribute('aria-labelledby') && document.getElementById(el.getAttribute('aria-labelledby'))?.textContent
                        || el.getAttribute('title')
                        || el.getAttribute('alt')
                        || el.getAttribute('placeholder')
                        || el.textContent?.trim()
                        || el.value
                        || '';

                    if (name === '' || accName.trim() === name || accName.trim().startsWith(name)) {{
                        matches.push(el);
                    }}
                }}

                if (matches.length === 0) return null;
                return matches[Math.min(nthIdx, matches.length - 1)] || null;
            }})()"#
        );

        // Use raw CDP Runtime.evaluate to get RemoteObject with object_id
        let params = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;

        let result = self.inner.execute(params).await.map_err(Error::Cdp)?;

        // The CDP result contains a RemoteObject; extract its object_id
        result
            .result
            .result
            .object_id
            .ok_or_else(|| {
                Error::ElementNotFound(format!(
                    "element with role={role} name=\"{name}\" not found in DOM"
                ))
            })
    }

    /// Focus an element identified by a ref.
    async fn focus_ref_element(&self, r: &Ref) -> Result<()> {
        // Try fast path first
        if r.backend_node_id != 0 {
            let focus_result = self
                .inner
                .execute(FocusParams {
                    node_id: None,
                    backend_node_id: Some(BackendNodeId::new(r.backend_node_id)),
                    object_id: None,
                })
                .await;
            if focus_result.is_ok() {
                return Ok(());
            }
        }

        // Fallback: resolve via role+name, then focus via JS
        let object_id = self.resolve_by_role_name(&r.role, &r.name, r.nth).await?;
        self.inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(object_id)
                    .function_declaration("function() { this.focus(); }")
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Get the center coordinates of an element via its `RemoteObjectId`.
    async fn get_center_from_object(
        &self,
        object_id: chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId,
    ) -> Result<Point> {
        let result = self
            .inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(object_id)
                    .function_declaration(
                        "function() { \
                            const r = this.getBoundingClientRect(); \
                            return JSON.stringify({x: r.x + r.width/2, y: r.y + r.height/2}); \
                        }",
                    )
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;

        let json_str: String = result
            .into_value()
            .map_err(|_| Error::ElementNotFound("failed to get bounding rect".into()))?;

        let coords: serde_json::Value = serde_json::from_str(&json_str)?;
        let x = coords["x"].as_f64().unwrap_or(0.0);
        let y = coords["y"].as_f64().unwrap_or(0.0);

        Ok(Point { x, y })
    }

    /// Resolve a backend node id to a CDP `RemoteObjectId`.
    async fn resolve_to_object(
        &self,
        backend_node_id: i64,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let result = self
            .inner
            .execute(ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(BackendNodeId::new(backend_node_id)),
                object_group: Some("brother".to_owned()),
                execution_context_id: None,
            })
            .await
            .map_err(Error::Cdp)?;

        result
            .result
            .object
            .object_id
            .ok_or_else(|| Error::ElementNotFound("node has no object id".into()))
    }

    /// Find an element by CSS selector.
    async fn find_element(&self, selector: &str) -> Result<chromiumoxide::element::Element> {
        self.inner
            .find_element(selector)
            .await
            .map_err(|_| Error::ElementNotFound(format!("selector \"{selector}\" not found")))
    }

    /// Get the current history index.
    async fn current_history_index(&self) -> Result<usize> {
        let result = self
            .inner
            .execute(GetNavigationHistoryParams::default())
            .await
            .map_err(Error::Cdp)?;

        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let idx = result.result.current_index as usize;
        Ok(idx)
    }
}
