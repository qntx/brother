//! Query methods, DOM manipulation, and element utilities.

use std::time::Duration;

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::input::DispatchMouseEventType;
use chromiumoxide::cdp::js_protocol::runtime::CallFunctionOnParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Get text content of the page or a specific element.
    pub async fn get_text(&self, target: Option<&str>) -> Result<String> {
        if let Some(t) = target {
            self.call_text_on_target(t, "function() { return this.textContent || ''; }")
                .await
        } else {
            let val = self.eval("document.body?.innerText || ''").await?;
            Ok(val.as_str().unwrap_or("").to_owned())
        }
    }

    /// Get inner text (rendered) of an element.
    pub async fn get_inner_text(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerText || ''; }")
            .await
    }

    /// Get the current page URL.
    pub async fn url(&self) -> Result<String> {
        let url = self.inner.url().await.map_err(Error::Cdp)?;
        Ok(url.unwrap_or_default())
    }

    /// Get the current page title.
    pub async fn title(&self) -> Result<String> {
        let r = self
            .inner
            .evaluate("document.title")
            .await
            .map_err(Error::Cdp)?;
        Ok(r.into_value::<String>().unwrap_or_default())
    }

    /// Get the full page HTML content.
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(Error::Cdp)
    }

    /// Get the inner HTML of an element.
    pub async fn get_html(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerHTML || ''; }")
            .await
    }

    /// Get the value of an input element.
    pub async fn get_value(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.value || ''; }")
            .await
    }

    /// Get an attribute value from an element.
    pub async fn get_attribute(&self, target: &str, attribute: &str) -> Result<String> {
        let escaped = attribute.replace('\\', "\\\\").replace('\'', "\\'");
        self.call_text_on_target(
            target,
            &format!("function() {{ return this.getAttribute('{escaped}') || ''; }}"),
        )
        .await
    }

    /// Check if an element is visible (has layout and non-zero size).
    pub async fn is_visible(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(
            target,
            "function() { const r = this.getBoundingClientRect(); \
             return r.width > 0 && r.height > 0 && \
             getComputedStyle(this).visibility !== 'hidden'; }",
        )
        .await
    }

    /// Check if an element is enabled (not disabled).
    pub async fn is_enabled(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !this.disabled; }")
            .await
    }

    /// Check if a checkbox/radio is checked.
    pub async fn is_checked(&self, target: &str) -> Result<bool> {
        self.call_bool_on_target(target, "function() { return !!this.checked; }")
            .await
    }

    /// Count elements matching a CSS selector.
    pub async fn count(&self, selector: &str) -> Result<usize> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let val = self
            .eval(&format!("document.querySelectorAll('{escaped}').length"))
            .await?;
        Ok(usize::try_from(val.as_u64().unwrap_or(0)).unwrap_or(0))
    }

    /// Get computed styles of an element as a JSON value.
    pub async fn get_styles(&self, target: &str) -> Result<serde_json::Value> {
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            "(() => {{\
                const el = document.querySelector('{escaped}');\
                if (!el) throw new Error('element not found: {escaped}');\
                const s = getComputedStyle(el);\
                const r = el.getBoundingClientRect();\
                return {{\
                    tag: el.tagName.toLowerCase(),\
                    text: (el.innerText || \"\").trim().slice(0, 80) || null,\
                    box: {{ x: Math.round(r.x), y: Math.round(r.y), width: Math.round(r.width), height: Math.round(r.height) }},\
                    styles: {{\
                        fontSize: s.fontSize,\
                        fontWeight: s.fontWeight,\
                        fontFamily: s.fontFamily.split(\",\")[0].trim().replace(/\"/g, \"\"),\
                        color: s.color,\
                        backgroundColor: s.backgroundColor,\
                        borderRadius: s.borderRadius,\
                        border: s.border !== \"none\" && s.borderWidth !== \"0px\" ? s.border : null,\
                        boxShadow: s.boxShadow !== \"none\" ? s.boxShadow : null,\
                        padding: s.padding,\
                    }},\
                }};\
            }})()"
        );
        self.eval(&js).await
    }

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

    /// Get the bounding box (x, y, width, height) of an element.
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
