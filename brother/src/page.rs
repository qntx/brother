//! High-level page abstraction for browser interaction.
//!
//! All interaction methods accept a **target** string that is either:
//! - A ref from a prior snapshot: `"@e1"`, `"e1"`, or `"ref=e1"`
//! - A CSS selector: `"#submit"`, `".btn-primary"`

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::accessibility::GetFullAxTreeParams;
use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, FocusParams, ResolveNodeParams};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton,
};
use chromiumoxide::cdp::browser_protocol::page::{
    CaptureScreenshotFormat, GetNavigationHistoryParams, NavigateToHistoryEntryParams,
};
use chromiumoxide::cdp::js_protocol::runtime::{CallFunctionOnParams, EvaluateParams};
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::protocol::ScrollDirection;
use crate::snapshot::{self, Ref, RefMap, Snapshot, SnapshotOptions};

/// A browser page with ref-based interaction support.
///
/// Wraps a `chromiumoxide::Page` and adds accessibility snapshot / ref
/// tracking on top. All interaction methods accept a unified **target**
/// string — either a ref (`@e1`) or a CSS selector (`#id`).
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
    // Interaction (unified target: ref or CSS selector)
    // -----------------------------------------------------------------------

    /// Click an element by ref or CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn click(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.click(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Double-click an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn dblclick(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        // Two rapid clicks via CDP Input domain
        for _ in 0..2 {
            self.dispatch_mouse(DispatchMouseEventType::MousePressed, center, 1)
                .await?;
            self.dispatch_mouse(DispatchMouseEventType::MouseReleased, center, 1)
                .await?;
        }
        Ok(())
    }

    /// Clear and fill an input by ref or CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn fill(&self, target: &str, value: &str) -> Result<()> {
        self.focus(target).await?;
        self.key_press("Control+a").await?;
        self.key_press("Delete").await?;
        self.type_text(value).await
    }

    /// Focus an element, then type text (appends, does not clear).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn type_into(&self, target: &str, text: &str) -> Result<()> {
        self.focus(target).await?;
        self.type_text(text).await
    }

    /// Focus an element by ref or CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn focus(&self, target: &str) -> Result<()> {
        if let Some(r) = self.try_resolve_ref(target).await {
            return self.focus_ref_element(&r).await;
        }
        // CSS selector fallback: click to focus
        let el = self.find_element(target).await?;
        el.click().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Hover an element by ref or CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn hover(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.move_mouse(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Select a dropdown option by value.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn select_option(&self, target: &str, value: &str) -> Result<()> {
        let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");
        self.call_on_target(
            target,
            &format!(
                "function() {{ this.value = '{escaped_val}'; \
                 this.dispatchEvent(new Event('change', {{bubbles: true}})); }}"
            ),
        )
        .await?;
        Ok(())
    }

    /// Check a checkbox (no-op if already checked).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn check(&self, target: &str) -> Result<()> {
        self.call_on_target(target, "function() { if (!this.checked) this.click(); }")
            .await?;
        Ok(())
    }

    /// Uncheck a checkbox (no-op if already unchecked).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn uncheck(&self, target: &str) -> Result<()> {
        self.call_on_target(target, "function() { if (this.checked) this.click(); }")
            .await?;
        Ok(())
    }

    /// Scroll the page or a specific element.
    ///
    /// # Errors
    ///
    /// Returns an error if the scroll JS fails.
    pub async fn scroll(
        &self,
        direction: ScrollDirection,
        pixels: i64,
        target: Option<&str>,
    ) -> Result<()> {
        let (dx, dy) = match direction {
            ScrollDirection::Down => (0, pixels),
            ScrollDirection::Up => (0, -pixels),
            ScrollDirection::Right => (pixels, 0),
            ScrollDirection::Left => (-pixels, 0),
        };

        if let Some(t) = target {
            let escaped = t.replace('\\', "\\\\").replace('\'', "\\'");
            self.eval(&format!(
                "document.querySelector('{escaped}')?.scrollBy({dx},{dy})"
            ))
            .await?;
        } else {
            self.eval(&format!("window.scrollBy({dx},{dy})")).await?;
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Query
    // -----------------------------------------------------------------------

    /// Get text content of the page or a specific element.
    ///
    /// # Errors
    ///
    /// Returns an error if evaluation fails.
    pub async fn get_text(&self, target: Option<&str>) -> Result<String> {
        if let Some(t) = target {
            self.call_text_on_target(
                t,
                "function() { return this.innerText || this.textContent || ''; }",
            )
            .await
        } else {
            let val = self.eval("document.body?.innerText || ''").await?;
            Ok(val.as_str().unwrap_or("").to_owned())
        }
    }

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
        let r = self
            .inner
            .evaluate("document.title")
            .await
            .map_err(Error::Cdp)?;
        Ok(r.into_value::<String>().unwrap_or_default())
    }

    /// Get the full page HTML.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(Error::Cdp)
    }

    /// Get `innerHTML` of an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_html(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerHTML || ''; }")
            .await
    }

    /// Get the `value` property of an input element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_value(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.value || ''; }")
            .await
    }

    /// Get an attribute of an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_attribute(&self, target: &str, attribute: &str) -> Result<String> {
        let escaped = attribute.replace('\\', "\\\\").replace('\'', "\\'");
        self.call_text_on_target(
            target,
            &format!("function() {{ return this.getAttribute('{escaped}') || ''; }}"),
        )
        .await
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

    /// Capture a JPEG screenshot.
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
    // JavaScript
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

    /// Evaluate JS and deserialize the result.
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

    /// Press a key combo (e.g. `"Enter"`, `"Tab"`, `"Control+a"`).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn key_press(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyDown, key)
            .await?;
        self.dispatch_key(DispatchKeyEventType::KeyUp, key).await
    }

    /// Type text character by character.
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

    /// Wait for a CSS selector to appear in the DOM.
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<()> {
        self.poll_js(
            &format!("!!document.querySelector('{selector}')"),
            timeout,
            &format!("selector \"{selector}\" not found"),
        )
        .await
    }

    /// Wait for text to appear on the page.
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_text(&self, text: &str, timeout: Duration) -> Result<()> {
        let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
        self.poll_js(
            &format!("document.body?.innerText?.includes('{escaped}')"),
            timeout,
            &format!("text \"{text}\" not found"),
        )
        .await
    }

    /// Wait for the URL to contain a pattern.
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_url(&self, pattern: &str, timeout: Duration) -> Result<()> {
        let escaped = pattern.replace('\\', "\\\\").replace('\'', "\\'");
        self.poll_js(
            &format!("location.href.includes('{escaped}')"),
            timeout,
            &format!("URL pattern \"{pattern}\" not matched"),
        )
        .await
    }

    /// Wait for a `JavaScript` expression to return truthy.
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_function(&self, expression: &str, timeout: Duration) -> Result<()> {
        self.poll_js(expression, timeout, expression).await
    }

    /// Wait for network idle (no in-flight requests for 500 ms).
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_network_idle(&self, timeout: Duration) -> Result<()> {
        let inject = r"
            (() => {
                if (window.__brother_pending !== undefined) return;
                window.__brother_pending = 0;
                const F = window.fetch;
                window.fetch = function(...a) {
                    window.__brother_pending++;
                    return F.apply(this, a).finally(() => { window.__brother_pending--; });
                };
                const O = XMLHttpRequest.prototype.open;
                const S = XMLHttpRequest.prototype.send;
                XMLHttpRequest.prototype.open = function(...a) { this._b = true; return O.apply(this, a); };
                XMLHttpRequest.prototype.send = function(...a) {
                    if (this._b) {
                        window.__brother_pending++;
                        this.addEventListener('loadend', () => { window.__brother_pending--; }, {once:true});
                    }
                    return S.apply(this, a);
                };
            })()";
        let _ = self.inner.evaluate(inject.to_owned()).await;

        let deadline = tokio::time::Instant::now() + timeout;
        let mut quiet_since: Option<tokio::time::Instant> = None;
        let idle_ms = Duration::from_millis(500);

        loop {
            let pending: i64 = self
                .inner
                .evaluate("window.__brother_pending||0".to_owned())
                .await
                .map_err(Error::Cdp)?
                .into_value()
                .unwrap_or(0);

            if pending == 0 {
                let now = tokio::time::Instant::now();
                if now.duration_since(*quiet_since.get_or_insert(now)) >= idle_ms {
                    return Ok(());
                }
            } else {
                quiet_since = None;
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!(
                    "network not idle within {timeout:?} ({pending} pending)"
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

    /// Access the underlying `chromiumoxide::Page`.
    #[must_use]
    pub const fn inner(&self) -> &chromiumoxide::Page {
        &self.inner
    }

    // -----------------------------------------------------------------------
    // Internal: target resolution
    // -----------------------------------------------------------------------

    /// Check if a target string looks like a ref.
    fn is_ref(target: &str) -> bool {
        target.starts_with('@')
            || target.starts_with("ref=")
            || (target.starts_with('e')
                && target.len() > 1
                && target[1..].bytes().all(|b| b.is_ascii_digit()))
    }

    /// Normalize a ref id: strip `@` and `ref=` prefixes.
    fn normalize_ref(target: &str) -> &str {
        let s = target.strip_prefix('@').unwrap_or(target);
        s.strip_prefix("ref=").unwrap_or(s)
    }

    /// Try to resolve a target as a ref. Returns `None` if not a ref or not found.
    async fn try_resolve_ref(&self, target: &str) -> Option<Ref> {
        if !Self::is_ref(target) {
            return None;
        }
        let id = Self::normalize_ref(target);
        self.refs.lock().await.get(id).cloned()
    }

    /// Resolve any target (ref or CSS) to a `RemoteObjectId`.
    async fn resolve_target_object(
        &self,
        target: &str,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        if let Some(r) = self.try_resolve_ref(target).await {
            return self.resolve_ref_to_object(&r).await;
        }
        // CSS selector: find via JS and get the raw RemoteObject
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!("document.querySelector('{escaped}')");
        let params = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let result = self.inner.execute(params).await.map_err(Error::Cdp)?;
        result
            .result
            .result
            .object_id
            .ok_or_else(|| Error::ElementNotFound(format!("selector \"{target}\" not found")))
    }

    /// Resolve any target to its center point for click/hover.
    async fn resolve_target_center(&self, target: &str) -> Result<Point> {
        let oid = self.resolve_target_object(target).await?;
        self.get_center_from_object(oid).await
    }

    /// Call a JS function on a target element and discard the result.
    async fn call_on_target(&self, target: &str, function: &str) -> Result<()> {
        let oid = self.resolve_target_object(target).await?;
        self.inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(oid)
                    .function_declaration(function)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Call a JS function on a target and return the string result.
    async fn call_text_on_target(&self, target: &str, function: &str) -> Result<String> {
        let oid = self.resolve_target_object(target).await?;
        let result = self
            .inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(oid)
                    .function_declaration(function)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(result.into_value::<String>().unwrap_or_default())
    }

    // -----------------------------------------------------------------------
    // Internal: ref resolution (two-phase)
    // -----------------------------------------------------------------------

    /// Resolve a ref to a `RemoteObjectId` (fast path + JS fallback).
    async fn resolve_ref_to_object(
        &self,
        r: &Ref,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        if r.backend_node_id != 0 {
            if let Ok(oid) = self.resolve_backend_node(r.backend_node_id).await {
                return Ok(oid);
            }
            tracing::debug!(role = %r.role, name = %r.name, "backend_node_id stale, falling back to role+name");
        }
        self.resolve_by_role_name(&r.role, &r.name, r.nth).await
    }

    /// Focus a ref element (fast path + JS fallback).
    async fn focus_ref_element(&self, r: &Ref) -> Result<()> {
        if r.backend_node_id != 0 {
            let ok = self
                .inner
                .execute(FocusParams {
                    node_id: None,
                    backend_node_id: Some(BackendNodeId::new(r.backend_node_id)),
                    object_id: None,
                })
                .await;
            if ok.is_ok() {
                return Ok(());
            }
        }
        let oid = self.resolve_by_role_name(&r.role, &r.name, r.nth).await?;
        self.inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(oid)
                    .function_declaration("function() { this.focus(); }")
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Locate element by ARIA role + name via JS, returning `RemoteObjectId`.
    async fn resolve_by_role_name(
        &self,
        role: &str,
        name: &str,
        nth: Option<usize>,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let nth_idx = nth.unwrap_or(0);
        let esc_name = name.replace('\\', "\\\\").replace('\'', "\\'");
        let esc_role = role.replace('\\', "\\\\").replace('\'', "\\'");

        let js = format!(
            r#"(() => {{
                const R = {{
                    button: 'button,[role="button"],input[type="button"],input[type="submit"]',
                    link: 'a[href],[role="link"]',
                    textbox: 'input:not([type]),input[type="text"],input[type="email"],input[type="password"],input[type="search"],input[type="url"],input[type="tel"],input[type="number"],textarea,[role="textbox"],[contenteditable="true"]',
                    checkbox: 'input[type="checkbox"],[role="checkbox"]',
                    radio: 'input[type="radio"],[role="radio"]',
                    combobox: 'select,[role="combobox"]',
                    heading: 'h1,h2,h3,h4,h5,h6,[role="heading"]',
                    listbox: 'select[multiple],[role="listbox"]',
                    menuitem: '[role="menuitem"]',
                    option: 'option,[role="option"]',
                    slider: 'input[type="range"],[role="slider"]',
                    switch: '[role="switch"]',
                    tab: '[role="tab"]',
                    searchbox: 'input[type="search"],[role="searchbox"]',
                    spinbutton: 'input[type="number"],[role="spinbutton"]',
                }};
                const sel = R['{esc_role}'] || '[role="{esc_role}"]';
                const m = [];
                for (const el of document.querySelectorAll(sel)) {{
                    const n = el.getAttribute('aria-label')
                        || el.getAttribute('title')
                        || el.getAttribute('alt')
                        || el.getAttribute('placeholder')
                        || el.textContent?.trim()
                        || el.value || '';
                    if ('{esc_name}' === '' || n.trim() === '{esc_name}' || n.trim().startsWith('{esc_name}'))
                        m.push(el);
                }}
                return m[Math.min({nth_idx}, m.length - 1)] || null;
            }})()"#
        );

        let params = EvaluateParams::builder()
            .expression(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let result = self.inner.execute(params).await.map_err(Error::Cdp)?;
        result.result.result.object_id.ok_or_else(|| {
            Error::ElementNotFound(format!(
                "element role={role} name=\"{name}\" not found in DOM"
            ))
        })
    }

    // -----------------------------------------------------------------------
    // Internal: CDP primitives
    // -----------------------------------------------------------------------

    /// Resolve a backend node ID to `RemoteObjectId`.
    async fn resolve_backend_node(
        &self,
        id: i64,
    ) -> Result<chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId> {
        let result = self
            .inner
            .execute(ResolveNodeParams {
                node_id: None,
                backend_node_id: Some(BackendNodeId::new(id)),
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

    /// Get center point of an element from its `RemoteObjectId`.
    async fn get_center_from_object(
        &self,
        oid: chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId,
    ) -> Result<Point> {
        let result = self
            .inner
            .evaluate_function(
                CallFunctionOnParams::builder()
                    .object_id(oid)
                    .function_declaration(
                        "function(){const r=this.getBoundingClientRect();\
                         return JSON.stringify({x:r.x+r.width/2,y:r.y+r.height/2})}",
                    )
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;

        let s: String = result
            .into_value()
            .map_err(|_| Error::ElementNotFound("failed to get bounding rect".into()))?;
        let v: serde_json::Value = serde_json::from_str(&s)?;
        Ok(Point {
            x: v["x"].as_f64().unwrap_or(0.0),
            y: v["y"].as_f64().unwrap_or(0.0),
        })
    }

    /// Find element by CSS selector.
    async fn find_element(&self, selector: &str) -> Result<chromiumoxide::element::Element> {
        self.inner
            .find_element(selector)
            .await
            .map_err(|_| Error::ElementNotFound(format!("selector \"{selector}\" not found")))
    }

    /// Get current navigation history index.
    async fn current_history_index(&self) -> Result<usize> {
        let r = self
            .inner
            .execute(GetNavigationHistoryParams::default())
            .await
            .map_err(Error::Cdp)?;
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let idx = r.result.current_index as usize;
        Ok(idx)
    }

    /// Dispatch a key event.
    async fn dispatch_key(&self, kind: DispatchKeyEventType, key: &str) -> Result<()> {
        self.inner
            .execute(
                DispatchKeyEventParams::builder()
                    .r#type(kind)
                    .key(key)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Dispatch a mouse event at a point.
    async fn dispatch_mouse(
        &self,
        kind: DispatchMouseEventType,
        point: Point,
        click_count: i64,
    ) -> Result<()> {
        self.inner
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(kind)
                    .x(point.x)
                    .y(point.y)
                    .button(MouseButton::Left)
                    .click_count(click_count)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Poll a JS expression until truthy or timeout.
    async fn poll_js(&self, expr: &str, timeout: Duration, desc: &str) -> Result<()> {
        let deadline = tokio::time::Instant::now() + timeout;
        let wrapped = format!("!!({expr})");
        loop {
            let ok: bool = self
                .inner
                .evaluate(wrapped.clone())
                .await
                .map_err(Error::Cdp)?
                .into_value()
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(Error::Timeout(format!("{desc} within {timeout:?}")));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}
