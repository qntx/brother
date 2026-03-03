//! Navigation and wait methods.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::page::NavigateToHistoryEntryParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Navigate to a URL and wait for the page to load.
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.inner.goto(url).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go back in history.
    pub async fn go_back(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        let entry_id = i64::try_from(idx.saturating_sub(1)).unwrap_or(0);
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go forward in history.
    pub async fn go_forward(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        let entry_id = i64::try_from(idx + 1).unwrap_or(i64::MAX);
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Reload the current page.
    pub async fn reload(&self) -> Result<()> {
        self.inner.reload().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Wait for navigation to complete (network idle heuristic).
    pub async fn wait_for_navigation(&self) -> Result<()> {
        self.inner.wait_for_navigation().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Wait for a CSS selector to appear in the DOM.
    pub async fn wait_for_selector(&self, selector: &str, timeout: Duration) -> Result<()> {
        self.poll_js(
            &format!("!!document.querySelector('{selector}')"),
            timeout,
            &format!("selector \"{selector}\" not found"),
        )
        .await
    }

    /// Wait for text to appear on the page.
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
    pub async fn wait_for_function(&self, expression: &str, timeout: Duration) -> Result<()> {
        self.poll_js(expression, timeout, expression).await
    }

    /// Wait for network idle (no in-flight requests for 500 ms).
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
