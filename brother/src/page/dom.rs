//! DOM manipulation, file interaction, and element action utilities.

use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::input::DispatchMouseEventType;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Select all text in an element via JS `Selection` API.
    pub async fn select_all_text(&self, target: &str) -> Result<()> {
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const el = document.querySelector('{escaped}');
                if (!el) throw new Error('element not found: {escaped}');
                const range = document.createRange();
                range.selectNodeContents(el);
                const sel = window.getSelection();
                sel.removeAllRanges();
                sel.addRange(range);
            }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }

    /// Select the nth element (0-based, -1 = last) and optionally act on it.
    ///
    /// Sub-actions: `click`, `fill`, `check`, `hover`, `text`.
    pub async fn nth_action(
        &self,
        selector: &str,
        index: i64,
        subaction: Option<&str>,
        fill_value: Option<&str>,
    ) -> Result<serde_json::Value> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");

        // Build JS to resolve the nth element
        let resolve = format!(
            r"const els = document.querySelectorAll('{escaped}');
            const idx = {index} < 0 ? els.length + {index} : {index};
            if (idx < 0 || idx >= els.length) throw new Error('index {index} out of range, found ' + els.length + ' elements');
            const el = els[idx];"
        );

        let action_body = match subaction {
            None => format!(
                r"{resolve}
                return {{ tag: el.tagName.toLowerCase(), text: el.textContent.trim().substring(0, 100), index: idx, total: els.length }};"
            ),
            Some("click") => format!(
                r"{resolve}
                el.scrollIntoView({{ block: 'center' }});
                el.click();
                return {{ action: 'click', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};"
            ),
            Some("fill") => {
                let fv = fill_value.unwrap_or("");
                let efv = fv.replace('\\', "\\\\").replace('\'', "\\'");
                format!(
                    r"{resolve}
                    el.scrollIntoView({{ block: 'center' }});
                    el.focus();
                    el.value = '';
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.value = '{efv}';
                    el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    return {{ action: 'fill', tag: el.tagName.toLowerCase(), value: '{efv}' }};"
                )
            }
            Some("check") => format!(
                r"{resolve}
                if (!el.checked) el.click();
                return {{ action: 'check', tag: el.tagName.toLowerCase(), checked: el.checked }};"
            ),
            Some("hover") => format!(
                r"{resolve}
                el.scrollIntoView({{ block: 'center' }});
                el.dispatchEvent(new MouseEvent('mouseover', {{ bubbles: true }}));
                el.dispatchEvent(new MouseEvent('mouseenter', {{ bubbles: true }}));
                return {{ action: 'hover', tag: el.tagName.toLowerCase(), text: (el.textContent || '').trim().substring(0, 80) }};"
            ),
            Some("text") => format!(
                r"{resolve}
                return {{ action: 'text', text: (el.textContent || '').trim() }};"
            ),
            Some(other) => {
                return Err(Error::InvalidArgument(format!(
                    "unknown nth subaction '{other}'. Use: click, fill, check, hover, text"
                )));
            }
        };

        let js = format!("(async () => {{ {action_body} }})()");
        self.eval(&js).await
    }

    /// Highlight an element using CDP `Overlay.highlightNode`.
    pub async fn highlight(&self, target: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::dom::Rgba;
        use chromiumoxide::cdp::browser_protocol::overlay::{HighlightConfig, HighlightNodeParams};

        let object_id = self.resolve_target_object(target).await?;

        let config = HighlightConfig {
            content_color: Some(Rgba {
                r: 111,
                g: 168,
                b: 220,
                a: Some(0.66),
            }),
            border_color: Some(Rgba {
                r: 255,
                g: 229,
                b: 153,
                a: Some(0.66),
            }),
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
        use chromiumoxide::cdp::browser_protocol::dom::{
            DescribeNodeParams, SetFileInputFilesParams,
        };
        let object_id = self.resolve_target_object(target).await?;
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
    pub async fn drag(&self, source: &str, target: &str) -> Result<()> {
        let src = self.resolve_target_center(source).await?;
        let dst = self.resolve_target_center(target).await?;
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, src, 1)
            .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.dispatch_mouse(DispatchMouseEventType::MouseMoved, dst, 0)
            .await?;
        tokio::time::sleep(Duration::from_millis(50)).await;
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, dst, 1)
            .await?;
        Ok(())
    }

    /// Clear an input field by filling it with an empty string.
    pub async fn clear(&self, target: &str) -> Result<()> {
        self.fill(target, "").await
    }

    /// Scroll an element into the visible viewport.
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

    /// Set the page HTML content directly.
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

    /// Export the page as PDF and write to the given path (headless only).
    pub async fn pdf(&self, path: &str) -> Result<()> {
        self.pdf_with(path, None, None).await
    }

    /// Export the page as PDF with optional paper dimensions (in inches).
    ///
    /// Default is US Letter (8.5 × 11 inches).
    pub async fn pdf_with(
        &self,
        path: &str,
        paper_width: Option<f64>,
        paper_height: Option<f64>,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::PrintToPdfParams;
        let mut params = PrintToPdfParams::default();
        if let Some(w) = paper_width {
            params.paper_width = Some(w);
        }
        if let Some(h) = paper_height {
            params.paper_height = Some(h);
        }
        let resp = self.inner.execute(params).await.map_err(Error::Cdp)?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&resp.result.data)
            .map_err(|e| Error::Browser(format!("base64 decode: {e}")))?;
        tokio::fs::write(path, bytes)
            .await
            .map_err(|e| Error::Browser(format!("write PDF: {e}")))?;
        Ok(())
    }
}
