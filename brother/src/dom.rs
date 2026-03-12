//! DOM manipulation: upload, drag, highlight, set_content, PDF, script/style
//! injection, event dispatch, scroll-into-view, clear.

use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::input::DispatchMouseEventType;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

use crate::error::{Error, Result};
use crate::page::Page;

impl Page {
    /// Select all text in an element via JS Selection API.
    pub async fn select_all_text(&self, target: &str) -> Result<()> {
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!(
            r"(() => {{ const el = document.querySelector('{escaped}'); if (!el) throw new Error('element not found: {escaped}'); const range = document.createRange(); range.selectNodeContents(el); const sel = window.getSelection(); sel.removeAllRanges(); sel.addRange(range); }})()"
        )).await?;
        Ok(())
    }

    /// Highlight an element using CDP `Overlay.highlightNode`.
    pub async fn highlight(&self, target: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::dom::Rgba;
        use chromiumoxide::cdp::browser_protocol::overlay::{HighlightConfig, HighlightNodeParams};
        let object_id = self.resolve_target_object(target).await?;
        let config = HighlightConfig {
            content_color: Some(Rgba { r: 111, g: 168, b: 220, a: Some(0.66) }),
            border_color: Some(Rgba { r: 255, g: 229, b: 153, a: Some(0.66) }),
            show_info: Some(true),
            ..Default::default()
        };
        let mut params = HighlightNodeParams::new(config);
        params.object_id = Some(object_id);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Upload files to a `<input type="file">` element.
    pub async fn upload(&self, target: &str, files: &[String]) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::dom::{DescribeNodeParams, SetFileInputFilesParams};
        let object_id = self.resolve_target_object(target).await?;
        let desc = self.inner.execute(DescribeNodeParams { object_id: Some(object_id), ..Default::default() }).await.map_err(Error::Cdp)?;
        let mut params = SetFileInputFilesParams::new(files.to_vec());
        params.backend_node_id = Some(desc.result.node.backend_node_id);
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Drag one element onto another.
    pub async fn drag(&self, source: &str, target: &str) -> Result<()> {
        let src = self.resolve_target_center(source).await?;
        let dst = self.resolve_target_center(target).await?;
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, src, 1).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, dst, 0).await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, dst, 1).await?;
        Ok(())
    }

    /// Clear an input field by filling it with an empty string.
    pub async fn clear(&self, target: &str) -> Result<()> {
        self.fill(target, "").await
    }

    /// Scroll an element into the visible viewport.
    pub async fn scroll_into_view(&self, target: &str) -> Result<()> {
        let object_id = self.resolve_target_object(target).await?;
        let params = CallFunctionOnParams::builder()
            .object_id(object_id)
            .function_declaration("function(){this.scrollIntoView({block:'center',inline:'center'})}")
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Set the page HTML content directly.
    pub async fn set_content(&self, html: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::SetDocumentContentParams;
        let frame_id = self.inner.mainframe().await.map_err(Error::Cdp)?
            .ok_or_else(|| Error::Navigation("no main frame".into()))?;
        self.inner.execute(SetDocumentContentParams::new(frame_id, html.to_owned())).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Export the page as PDF and write to the given path (headless only).
    pub async fn pdf(&self, path: &str) -> Result<()> {
        self.pdf_with(path, None, None).await
    }

    /// Export the page as PDF with optional paper dimensions (in inches).
    pub async fn pdf_with(&self, path: &str, paper_width: Option<f64>, paper_height: Option<f64>) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
        let mut params = PrintToPdfParams::default();
        if let Some(w) = paper_width { params.paper_width = Some(w); }
        if let Some(h) = paper_height { params.paper_height = Some(h); }
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(&resp.result.data).map_err(|e| Error::Browser(format!("base64 decode: {e}")))?;
        tokio::fs::write(path, bytes).await.map_err(|e| Error::Browser(format!("write PDF: {e}")))?;
        Ok(())
    }

    /// Inject a `<script>` tag into the current page.
    pub async fn add_script(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
                format!(r"(() => {{ const s = document.createElement('script'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()")
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(r"(() => {{ const s = document.createElement('script'); s.src = '{escaped}'; document.head.appendChild(s); }})()")
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Inject a `<style>` or `<link>` tag into the current page.
    pub async fn add_style(&self, content: Option<&str>, url: Option<&str>) -> Result<()> {
        let js = match (content, url) {
            (Some(c), _) => {
                let escaped = c.replace('\\', "\\\\").replace('`', "\\`").replace('$', "\\$");
                format!(r"(() => {{ const s = document.createElement('style'); s.textContent = `{escaped}`; document.head.appendChild(s); }})()")
            }
            (_, Some(u)) => {
                let escaped = u.replace('\\', "\\\\").replace('\'', "\\'");
                format!(r"(() => {{ const l = document.createElement('link'); l.rel = 'stylesheet'; l.href = '{escaped}'; document.head.appendChild(l); }})()")
            }
            _ => return Err(Error::Browser("either content or url is required".into())),
        };
        self.eval(&js).await?;
        Ok(())
    }

    /// Dispatch a DOM event on an element.
    pub async fn dispatch_event(&self, target: &str, event: &str, event_init: Option<&str>) -> Result<()> {
        let escaped_event = event.replace('\\', "\\\\").replace('\'', "\\'");
        let init_arg = event_init.map_or_else(|| "{}".to_owned(), ToOwned::to_owned);
        let escaped_sel = target.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!(
            r"(() => {{ const el = document.querySelector('{escaped_sel}'); if (!el) throw new Error('element not found: {escaped_sel}'); el.dispatchEvent(new Event('{escaped_event}', {init_arg})); }})()"
        )).await?;
        Ok(())
    }
}
