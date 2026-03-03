//! High-level page abstraction for browser interaction.
//!
//! All interaction methods accept a **target** string that is either:
//! - A ref from a prior snapshot: `"@e1"`, `"e1"`, or `"ref=e1"`
//! - A CSS selector: `"#submit"`, `".btn-primary"`

mod cookie_storage;
mod emulation;
mod find;
mod interaction;
mod navigation;
mod query;
mod screenshot;
mod snapshot_cmd;

use std::sync::Arc;
use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::dom::{BackendNodeId, FocusParams, ResolveNodeParams};
use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton as CdpMouseButton,
};
use chromiumoxide::cdp::js_protocol::runtime::{
    CallFunctionOnParams, EvaluateParams, EventConsoleApiCalled, EventExceptionThrown,
};
use chromiumoxide::layout::Point;
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::snapshot::{Ref, RefMap};

/// Direction for scroll operations.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    /// Scroll down (positive Y).
    Down,
    /// Scroll up (negative Y).
    Up,
    /// Scroll right (positive X).
    Right,
    /// Scroll left (negative X).
    Left,
}

/// Mouse button for click and mouse commands.
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    /// Left mouse button (default).
    #[default]
    Left,
    /// Right mouse button.
    Right,
    /// Middle mouse button.
    Middle,
}

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

/// Structured cookie input for [`Page::set_cookies`].
///
/// All fields except `name` and `value` are optional. When `url` is omitted,
/// the current page URL is used automatically.
#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct CookieInput {
    /// Cookie name.
    pub name: String,
    /// Cookie value.
    pub value: String,
    /// URL to associate the cookie with (defaults to current page URL).
    #[serde(default)]
    pub url: Option<String>,
    /// Cookie domain.
    #[serde(default)]
    pub domain: Option<String>,
    /// Cookie path.
    #[serde(default)]
    pub path: Option<String>,
    /// Expiration as Unix timestamp in seconds. `None` = session cookie.
    #[serde(default)]
    pub expires: Option<f64>,
    /// Mark as HTTP-only.
    #[serde(default)]
    pub http_only: Option<bool>,
    /// Mark as secure (HTTPS only).
    #[serde(default)]
    pub secure: Option<bool>,
    /// `SameSite` policy: `"Strict"`, `"Lax"`, or `"None"`.
    #[serde(default)]
    pub same_site: Option<String>,
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

    /// Access the underlying `chromiumoxide::Page`.
    #[must_use]
    pub const fn inner(&self) -> &chromiumoxide::Page {
        &self.inner
    }

    // NOTE: For arbitrary CDP commands (tracing, profiler, etc.), use
    // `page.inner().execute(params)` directly. The `inner()` accessor
    // provides full access to chromiumoxide's typed CDP API.

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
    pub async fn resolve_target_center(&self, target: &str) -> Result<Point> {
        let oid = self.resolve_target_object(target).await?;
        self.get_center_from_object(oid).await
    }

    /// Execute a JS function on a `RemoteObjectId` and return the raw result.
    async fn call_fn_on(
        &self,
        oid: chromiumoxide::cdp::js_protocol::runtime::RemoteObjectId,
        function: &str,
    ) -> Result<Option<serde_json::Value>> {
        let params = CallFunctionOnParams::builder()
            .object_id(oid)
            .function_declaration(function)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(resp.result.result.value)
    }

    /// Call a JS function on a target element and discard the result.
    async fn call_on_target(&self, target: &str, function: &str) -> Result<()> {
        let oid = self.resolve_target_object(target).await?;
        self.call_fn_on(oid, function).await?;
        Ok(())
    }

    /// Call a JS function on a target and return a boolean result.
    async fn call_bool_on_target(&self, target: &str, function: &str) -> Result<bool> {
        let oid = self.resolve_target_object(target).await?;
        let val = self.call_fn_on(oid, function).await?;
        Ok(val.and_then(|v| v.as_bool()).unwrap_or(false))
    }

    /// Call a JS function on a target and return the string result.
    async fn call_text_on_target(&self, target: &str, function: &str) -> Result<String> {
        let oid = self.resolve_target_object(target).await?;
        let val = self.call_fn_on(oid, function).await?;
        Ok(val
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default())
    }

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
        use chromiumoxide::cdp::browser_protocol::page::GetNavigationHistoryParams;
        let r = self
            .inner
            .execute(GetNavigationHistoryParams::default())
            .await
            .map_err(Error::Cdp)?;
        let idx = usize::try_from(r.result.current_index).unwrap_or(0);
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

    /// Dispatch a mouse event at a point (left button).
    async fn dispatch_mouse(
        &self,
        kind: DispatchMouseEventType,
        point: Point,
        click_count: i64,
    ) -> Result<()> {
        self.dispatch_mouse_with(kind, point, click_count, CdpMouseButton::Left)
            .await
    }

    /// Dispatch a mouse event at a point with a specific button.
    async fn dispatch_mouse_with(
        &self,
        kind: DispatchMouseEventType,
        point: Point,
        click_count: i64,
        button: CdpMouseButton,
    ) -> Result<()> {
        self.inner
            .execute(
                DispatchMouseEventParams::builder()
                    .r#type(kind)
                    .x(point.x)
                    .y(point.y)
                    .button(button)
                    .click_count(click_count)
                    .build()
                    .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
            )
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Convert protocol `MouseButton` to CDP `MouseButton`.
    const fn to_cdp_button(button: MouseButton) -> CdpMouseButton {
        match button {
            MouseButton::Left => CdpMouseButton::Left,
            MouseButton::Right => CdpMouseButton::Right,
            MouseButton::Middle => CdpMouseButton::Middle,
        }
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

    /// Return all captured console messages and clear the buffer.
    pub async fn take_console_logs(&self) -> Vec<ConsoleEntry> {
        std::mem::take(&mut *self.console_logs.lock().await)
    }

    /// Return all captured JS errors and clear the buffer.
    pub async fn take_js_errors(&self) -> Vec<JsError> {
        std::mem::take(&mut *self.js_errors.lock().await)
    }

    /// Evaluate a `JavaScript` expression and return the raw result.
    pub async fn eval(&self, expression: &str) -> Result<serde_json::Value> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;
        Ok(result
            .into_value::<serde_json::Value>()
            .unwrap_or(serde_json::Value::Null))
    }

    /// Evaluate JS and deserialize the result.
    pub async fn eval_as<T: serde::de::DeserializeOwned>(&self, expression: &str) -> Result<T> {
        let result = self.inner.evaluate(expression).await.map_err(Error::Cdp)?;
        result
            .into_value()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e.to_string())))
    }

    /// Get the most recent dialog info (if a dialog is open).
    pub async fn dialog_message(&self) -> Option<DialogInfo> {
        self.dialog.lock().await.clone()
    }

    /// Accept (OK) the current `JavaScript` dialog, optionally providing prompt text.
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
    pub async fn dialog_dismiss(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
        self.inner
            .execute(HandleJavaScriptDialogParams::new(false))
            .await
            .map_err(Error::Cdp)?;
        *self.dialog.lock().await = None;
        Ok(())
    }
}
