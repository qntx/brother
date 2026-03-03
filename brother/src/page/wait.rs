//! Wait methods: `wait_for_selector`, `wait_for_text`, `wait_for_url`, `wait_for_function`, etc.

use std::time::Duration;

use crate::error::{Error, Result};

use super::Page;

impl Page {
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
}
