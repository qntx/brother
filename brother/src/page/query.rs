//! Read-only query methods: text, attributes, state checks, styles, bounding box.

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
}
