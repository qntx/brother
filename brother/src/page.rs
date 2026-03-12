//! Page struct and core methods.
//!
//! [`Page`] wraps a `chromiumoxide::Page` with accessibility snapshot / ref
//! tracking. All interaction methods accept a unified **target** string —
//! either a ref (`@e1`) or a CSS selector (`#id`).
//!
//! Domain-specific `impl Page` blocks live in sibling modules:
//! [`resolve`](crate::resolve), [`navigate`](crate::navigate),
//! [`interact`](crate::interact), [`observe`](crate::observe),
//! [`emulate`](crate::emulate), [`storage`](crate::storage),
//! [`dom`](crate::dom).

use std::sync::Arc;

use chromiumoxide::cdp::js_protocol::runtime::{
    EventConsoleApiCalled, EventExceptionThrown,
};
use futures::StreamExt;
use serde::Serialize;
use tokio::sync::Mutex;

use crate::error::{Error, Result};
use crate::snapshot::RefMap;

// ---------------------------------------------------------------------------
// Page observation types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Page struct
// ---------------------------------------------------------------------------

type LogBuf<T> = Arc<Mutex<Vec<T>>>;

const MAX_LOG_ENTRIES: usize = 500;

/// A browser page with ref-based interaction support.
#[derive(Debug, Clone)]
pub struct Page {
    pub(crate) inner: chromiumoxide::Page,
    /// Cached refs from the most recent snapshot.
    pub(crate) refs: Arc<Mutex<RefMap>>,
    console_logs: LogBuf<ConsoleEntry>,
    js_errors: LogBuf<JsError>,
    dialog: Arc<Mutex<Option<DialogInfo>>>,
}

impl Page {
    /// Wrap a chromiumoxide page and start background event listeners
    /// for console messages, JS exceptions, and dialogs.
    pub(crate) fn new(inner: chromiumoxide::Page) -> Self {
        let console_logs: LogBuf<ConsoleEntry> = Arc::new(Mutex::new(Vec::new()));
        let js_errors: LogBuf<JsError> = Arc::new(Mutex::new(Vec::new()));

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
    ///
    /// Escape hatch for advanced CDP operations not covered by this API.
    #[must_use]
    pub const fn inner(&self) -> &chromiumoxide::Page {
        &self.inner
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
    pub async fn eval_as<T: serde::de::DeserializeOwned>(
        &self,
        expression: &str,
    ) -> Result<T> {
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
