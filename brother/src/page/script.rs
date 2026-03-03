//! Script and style injection, DOM event dispatch.

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Add a script to evaluate on every new document (before page JS).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn add_init_script(&self, script: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::AddScriptToEvaluateOnNewDocumentParams;
        let params = AddScriptToEvaluateOnNewDocumentParams::new(script.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Inject a `<script>` tag into the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails or neither content nor url is given.
    pub async fn add_script(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c
                    .replace('\\', "\\\\")
                    .replace('`', "\\`")
                    .replace('$', "\\$");
                format!(
                    r"(() => {{ const s = document.createElement('script'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
                )
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"(() => {{ const s = document.createElement('script'); s.src = '{escaped}'; document.head.appendChild(s); }})()"
                )
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Inject a `<style>` or `<link>` tag into the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails or neither content nor url is given.
    pub async fn add_style(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c
                    .replace('\\', "\\\\")
                    .replace('`', "\\`")
                    .replace('$', "\\$");
                format!(
                    r"(() => {{ const s = document.createElement('style'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()"
                )
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"(() => {{ const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = '{escaped}'; document.head.appendChild(l); }})()"
                )
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Dispatch a DOM event on an element.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn dispatch_event(
        &self,
        target: &str,
        event: &str,
        event_init: Option<&str>,
    ) -> Result<()> {
        let escaped_event = event.replace('\\', "\\\\").replace('\'', "\\'");
        let init_arg = event_init.map_or_else(|| "{}".to_owned(), ToOwned::to_owned);
        let escaped_sel = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const el = document.querySelector('{escaped_sel}');
                if (!el) throw new Error('element not found: {escaped_sel}');
                el.dispatchEvent(new Event('{escaped_event}', {init_arg}));
            }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }
}
