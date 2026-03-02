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
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EvaluateParams, EventConsoleApiCalled, EventExceptionThrown,
};
use chromiumoxide::layout::Point;
use chromiumoxide::page::ScreenshotParams;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::protocol::ScrollDirection;
use crate::snapshot::{self, CursorItem, Ref, RefMap, Snapshot, SnapshotOptions};

/// A captured console message from the browser.
#[derive(Debug, Clone, Serialize)]
pub struct ConsoleEntry {
    /// Log level: `log`, `warn`, `error`, `info`, `debug`, etc.
    pub level: String,
    /// Serialized message text.
    pub text: String,
}

/// A captured `JavaScript` error from the browser.
#[derive(Debug, Clone, Serialize)]
pub struct JsError {
    /// Error message text.
    pub message: String,
}

/// Shared log buffer (bounded to prevent unbounded growth).
type LogBuf<T> = Arc<Mutex<Vec<T>>>;

/// Maximum number of console/error entries to buffer per page.
const MAX_LOG_ENTRIES: usize = 500;

/// Cached dialog info from `Page.javascriptDialogOpening`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct DialogInfo {
    /// Dialog type: `alert`, `confirm`, `prompt`, `beforeunload`.
    pub dialog_type: String,
    /// The dialog message text.
    pub message: String,
    /// Default prompt value (for prompt dialogs).
    pub default_prompt: String,
}

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
    /// Console messages captured from `Runtime.consoleAPICalled`.
    console_logs: LogBuf<ConsoleEntry>,
    /// JS errors captured from `Runtime.exceptionThrown`.
    js_errors: LogBuf<JsError>,
    /// Most recent dialog info (if a dialog is open).
    dialog: Arc<Mutex<Option<DialogInfo>>>,
}

impl Page {
    /// Wrap a chromiumoxide page and start background event listeners
    /// for console messages and JS exceptions.
    pub(crate) fn new(inner: chromiumoxide::Page) -> Self {
        let console_logs: LogBuf<ConsoleEntry> = Arc::new(Mutex::new(Vec::new()));
        let js_errors: LogBuf<JsError> = Arc::new(Mutex::new(Vec::new()));

        // Spawn background listener for console.log/warn/error/...
        {
            let page = inner.clone();
            let buf = Arc::clone(&console_logs);
            tokio::spawn(async move {
                let Ok(mut stream) = page.event_listener::<EventConsoleApiCalled>().await else {
                    return;
                };
                while let Some(event) = stream.next().await {
                    let level = format!("{:?}", event.r#type).to_ascii_lowercase();
                    let text = event
                        .args
                        .iter()
                        .filter_map(|a| {
                            a.value
                                .as_ref()
                                .map(|v| v.to_string().trim_matches('"').to_owned())
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let mut guard = buf.lock().await;
                    if guard.len() < MAX_LOG_ENTRIES {
                        guard.push(ConsoleEntry { level, text });
                    }
                }
            });
        }

        // Spawn background listener for uncaught JS exceptions
        {
            let page = inner.clone();
            let buf = Arc::clone(&js_errors);
            tokio::spawn(async move {
                let Ok(mut stream) = page.event_listener::<EventExceptionThrown>().await else {
                    return;
                };
                while let Some(event) = stream.next().await {
                    let message = event
                        .exception_details
                        .exception
                        .as_ref()
                        .and_then(|e| e.description.clone())
                        .unwrap_or_else(|| event.exception_details.text.clone());
                    let mut guard = buf.lock().await;
                    if guard.len() < MAX_LOG_ENTRIES {
                        guard.push(JsError { message });
                    }
                }
            });
        }

        // Spawn background listener for JavaScript dialogs (alert/confirm/prompt)
        let dialog: Arc<Mutex<Option<DialogInfo>>> = Arc::new(Mutex::new(None));
        {
            let page = inner.clone();
            let buf = Arc::clone(&dialog);
            tokio::spawn(async move {
                use chromiumoxide::cdp::browser_protocol::page::EventJavascriptDialogOpening;
                let Ok(mut stream) = page.event_listener::<EventJavascriptDialogOpening>().await
                else {
                    return;
                };
                while let Some(event) = stream.next().await {
                    let info = DialogInfo {
                        dialog_type: format!("{:?}", event.r#type).to_ascii_lowercase(),
                        message: event.message.clone(),
                        default_prompt: event.default_prompt.clone().unwrap_or_default(),
                    };
                    *buf.lock().await = Some(info);
                }
            });
        }

        Self {
            inner,
            refs: Arc::new(Mutex::new(RefMap::new())),
            console_logs,
            js_errors,
            dialog,
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

        let mut snap = snapshot::build_snapshot(&nodes, &options);

        // Append cursor-interactive elements (cursor:pointer / onclick / tabindex)
        // that have no proper ARIA roles and were missed by the AX tree.
        if options.cursor_interactive {
            self.append_cursor_interactive_elements(&mut snap).await;
        }

        // Cache refs for subsequent ref-based interactions
        *self.refs.lock().await = snap.refs().clone();

        Ok(snap)
    }

    /// Detect elements with cursor:pointer / onclick / tabindex that lack ARIA
    /// roles and append them as extra refs to the snapshot.
    async fn append_cursor_interactive_elements(&self, snap: &mut Snapshot) {
        // JS finds elements that are cursor-interactive but not natively interactive
        let js = r"(() => {
            const interactive = new Set([
                'a','button','input','select','textarea','details','summary'
            ]);
            const results = [];
            for (const el of document.querySelectorAll('*')) {
                if (interactive.has(el.tagName.toLowerCase())) continue;
                const role = el.getAttribute('role');
                if (role && ['button','link','textbox','checkbox','radio',
                    'combobox','menuitem','option','tab','switch'].includes(role)) continue;
                const cs = getComputedStyle(el);
                const ptr = cs.cursor === 'pointer';
                const click = el.hasAttribute('onclick') || el.onclick !== null;
                const ti = el.getAttribute('tabindex');
                const tab = ti !== null && ti !== '-1';
                if (!ptr && !click && !tab) continue;
                if (ptr && !click && !tab) {
                    const p = el.parentElement;
                    if (p && getComputedStyle(p).cursor === 'pointer') continue;
                }
                const text = (el.textContent || '').trim().slice(0, 100);
                if (!text) continue;
                const r = el.getBoundingClientRect();
                if (r.width === 0 || r.height === 0) continue;
                const hints = [];
                if (ptr) hints.push('cursor:pointer');
                if (click) hints.push('onclick');
                if (tab) hints.push('tabindex');
                results.push({ text, hints: hints.join(', ') });
            }
            return JSON.stringify(results);
        })()";

        let Ok(val) = self.eval(js).await else {
            return;
        };
        let json_str = val.as_str().unwrap_or("[]");
        let Ok(items) = serde_json::from_str::<Vec<CursorItem>>(json_str) else {
            return;
        };

        snap.append_cursor_elements(&items);
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
    // State checks
    // -----------------------------------------------------------------------

    /// Check if an element is visible (has layout and non-zero size).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn is_visible(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(
            target,
            "function() { const r = this.getBoundingClientRect(); \
             return r.width > 0 && r.height > 0 && getComputedStyle(this).visibility !== 'hidden'; }",
        )
        .await
    }

    /// Check if an element is enabled (not disabled).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn is_enabled(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !this.disabled; }")
            .await
    }

    /// Check if a checkbox/radio is checked.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn is_checked(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !!this.checked; }")
            .await
    }

    /// Count elements matching a CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn count(&self, selector: &str) -> Result<usize> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self
            .eval(&format!("document.querySelectorAll('{escaped}').length"))
            .await?;
        #[allow(clippy::cast_possible_truncation)]
        Ok(val.as_u64().unwrap_or(0) as usize)
    }

    // -----------------------------------------------------------------------
    // Cookie / Storage
    // -----------------------------------------------------------------------

    /// Get all cookies for the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn get_cookies(&self) -> Result<serde_json::Value> {
        use chromiumoxide::cdp::browser_protocol::network::GetCookiesParams;
        let result = self
            .inner
            .execute(GetCookiesParams::default())
            .await
            .map_err(Error::Cdp)?;
        serde_json::to_value(&result.result.cookies)
            .map_err(|e| Error::Snapshot(format!("cookie serialize: {e}")))
    }

    /// Set a cookie via JS `document.cookie`.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_cookie(&self, cookie_str: &str) -> Result<()> {
        let escaped = cookie_str.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("document.cookie = '{escaped}'")).await?;
        Ok(())
    }

    /// Clear all cookies for the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn clear_cookies(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::network::{
            DeleteCookiesParams, GetCookiesParams,
        };
        let result = self
            .inner
            .execute(GetCookiesParams::default())
            .await
            .map_err(Error::Cdp)?;
        for cookie in &result.result.cookies {
            self.inner
                .execute(DeleteCookiesParams::new(cookie.name.clone()))
                .await
                .map_err(Error::Cdp)?;
        }
        Ok(())
    }

    /// Get a `localStorage` or `sessionStorage` item.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn get_storage(&self, key: &str, session: bool) -> Result<String> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        let escaped = key.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self
            .eval(&format!("{storage}.getItem('{escaped}')"))
            .await?;
        Ok(val.as_str().unwrap_or("").to_owned())
    }

    /// Set a `localStorage` or `sessionStorage` item.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_storage(&self, key: &str, value: &str, session: bool) -> Result<()> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        let ek = key.replace('\\', "\\\\").replace('\'', "\\'");
        let ev = value.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("{storage}.setItem('{ek}', '{ev}')"))
            .await?;
        Ok(())
    }

    /// Clear `localStorage` or `sessionStorage`.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn clear_storage(&self, session: bool) -> Result<()> {
        let storage = if session {
            "sessionStorage"
        } else {
            "localStorage"
        };
        self.eval(&format!("{storage}.clear()")).await?;
        Ok(())
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
    // Console / Error logs
    // -----------------------------------------------------------------------

    /// Return all captured console messages and clear the buffer.
    pub async fn take_console_logs(&self) -> Vec<ConsoleEntry> {
        std::mem::take(&mut *self.console_logs.lock().await)
    }

    /// Return all captured JS errors and clear the buffer.
    pub async fn take_js_errors(&self) -> Vec<JsError> {
        std::mem::take(&mut *self.js_errors.lock().await)
    }

    // -----------------------------------------------------------------------
    // Dialog handling
    // -----------------------------------------------------------------------

    /// Get the most recent dialog info (if a dialog is open).
    pub async fn dialog_message(&self) -> Option<DialogInfo> {
        self.dialog.lock().await.clone()
    }

    /// Accept (OK) the current `JavaScript` dialog, optionally providing
    /// prompt text.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn dialog_accept(&self, prompt_text: Option<&str>) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
        let mut params = HandleJavaScriptDialogParams::new(true);
        if let Some(text) = prompt_text {
            params.prompt_text = Some(text.to_owned());
        }
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        *self.dialog.lock().await = None;
        Ok(())
    }

    /// Dismiss (Cancel) the current `JavaScript` dialog.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn dialog_dismiss(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
        self.inner
            .execute(HandleJavaScriptDialogParams::new(false))
            .await
            .map_err(Error::Cdp)?;
        *self.dialog.lock().await = None;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Raw keyboard
    // -----------------------------------------------------------------------

    /// Press and hold a key (without releasing).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn key_down(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyDown, key).await
    }

    /// Release a held key.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn key_up(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyUp, key).await
    }

    /// Insert text directly without firing individual key events.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
        let params = InsertTextParams::new(text.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // File / DOM manipulation
    // -----------------------------------------------------------------------

    /// Upload files to a `<input type="file">` element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found or the CDP call fails.
    pub async fn upload(&self, target: &str, files: &[String]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::dom::{
            DescribeNodeParams, SetFileInputFilesParams,
        };

        let object_id = self.resolve_target_object(target).await?;

        // Resolve the DOM node (backendNodeId) from the remote object.
        let desc_params = DescribeNodeParams {
            object_id: Some(object_id),
            ..Default::default()
        };
        let desc = self.inner.execute(desc_params).await.map_err(Error::Cdp)?;
        let backend_node_id = desc.result.node.backend_node_id;

        let mut params = SetFileInputFilesParams::new(files.to_vec());
        params.backend_node_id = Some(backend_node_id);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Drag one element onto another.
    ///
    /// Simulates a full drag gesture: mousedown on source center, mousemove to
    /// target center, mouseup on target center.
    ///
    /// # Errors
    ///
    /// Returns an error if either element is not found.
    pub async fn drag(&self, source: &str, target: &str) -> Result<()> {
        let src = self.resolve_target_center(source).await?;
        let dst = self.resolve_target_center(target).await?;

        // mousedown on source
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, src, 1)
            .await?;
        // small pause to let drag start
        tokio::time::sleep(Duration::from_millis(50)).await;
        // mousemove to target
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, dst, 0)
            .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        // mouseup on target
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, dst, 1)
            .await?;
        Ok(())
    }

    /// Clear an input field by filling it with an empty string.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn clear(&self, target: &str) -> Result<()> {
        self.fill(target, "").await
    }

    /// Scroll an element into the visible viewport.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn scroll_into_view(&self, target: &str) -> Result<()> {
        let object_id = self.resolve_target_object(target).await?;
        let js = "function(){this.scrollIntoView({block:'center',inline:'center'})}";
        let params = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration(js)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Get the bounding box (x, y, width, height) of an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn bounding_box(&self, target: &str) -> Result<(f64, f64, f64, f64)> {
        let object_id = self.resolve_target_object(target).await?;
        let js = "function(){const r=this.getBoundingClientRect();return JSON.stringify({x:r.x,y:r.y,width:r.width,height:r.height})}";
        let params = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration(js)
            .return_by_value(true)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let json_str: String = resp
            .result
            .result
            .value
            .as_ref()
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| Error::Browser("bounding_box returned no value".into()))?;
        let parsed: serde_json::Value =
            serde_json::from_str(&json_str).map_err(|e| Error::Browser(e.to_string()))?;
        let x = parsed["x"].as_f64().unwrap_or(0.0);
        let y = parsed["y"].as_f64().unwrap_or(0.0);
        let w = parsed["width"].as_f64().unwrap_or(0.0);
        let h = parsed["height"].as_f64().unwrap_or(0.0);
        Ok((x, y, w, h))
    }

    /// Set the page HTML content directly.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn set_content(&self, html: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::SetDocumentContentParams;
        let frame_id = self
            .inner
            .mainframe()
            .await
            .map_err(Error::Cdp)?
            .ok_or_else(|| Error::Navigation("no main frame".into()))?;
        let params = SetDocumentContentParams::new(frame_id, html.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Export the page as PDF and write to the given path.
    ///
    /// Only works in headless mode.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails or writing fails.
    pub async fn pdf(&self, path: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
        let params = PrintToPdfParams::default();
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let bytes = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            &resp.result.data,
        )
        .map_err(|e| Error::Browser(format!("base64 decode: {e}")))?;
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| Error::Browser(format!("write PDF: {e}")))?;
        Ok(())
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
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(function)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Call a JS function on a target and return a boolean result.
    async fn call_bool_on_target(&self, target: &str, function: &str) -> Result<bool> {
        let oid = self.resolve_target_object(target).await?;
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(function)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(resp
            .result
            .result
            .value
            .and_then(|v| v.as_bool())
            .unwrap_or(false))
    }

    /// Call a JS function on a target and return the string result.
    async fn call_text_on_target(&self, target: &str, function: &str) -> Result<String> {
        let oid = self.resolve_target_object(target).await?;
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(function)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(resp
            .result
            .result
            .value
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default())
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
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration("function() { this.focus(); }")
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
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
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(
                "function(){const r=this.getBoundingClientRect();\
                 return JSON.stringify({x:r.x+r.width/2,y:r.y+r.height/2})}",
            )
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let s = resp
            .result
            .result
            .value
            .and_then(|v| v.as_str().map(String::from))
            .ok_or_else(|| Error::ElementNotFound("failed to get bounding rect".into()))?;
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
