//! Query methods: `get_text`, `get_inner_text`, `url`, `title`, `content`, `get_html`, `get_value`, `get_attribute`.

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Get text content of the page or a specific element.
    ///
    /// # Errors
    ///
    /// Returns an error if evaluation fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_inner_text(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerText || ''; }")
            .await
    }

    /// Get the current page URL.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn url(&self) -> Result<String> {
        let url = self.inner.url().await.map_err(Error::Cdp)?;
        Ok(url.unwrap_or_default())
    }

    /// Get the current page title.
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

    /// Get the full page HTML content.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn content(&self) -> Result<String> {
        self.inner.content().await.map_err(Error::Cdp)
    }

    /// Get the inner HTML of an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_html(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.innerHTML || ''; }")
            .await
    }

    /// Get the value of an input element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn get_value(&self, target: &str) -> Result<String> {
        self.call_text_on_target(target, "function() { return this.value || ''; }")
            .await
    }

    /// Get an attribute value from an element.
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

    /// Check if an element is visible (has layout and non-zero size).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
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

    /// Get computed styles of an element as a JSON value.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
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

    /// Get the nth element matching a CSS selector (0-indexed).
    ///
    /// Returns a JSON object with tag, text, and index.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn nth(&self, selector: &str, index: usize) -> Result<serde_json::Value> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const els = document.querySelectorAll('{escaped}');
                if ({index} >= els.length) throw new Error('index {index} out of range, found ' + els.length + ' elements');
                const el = els[{index}];
                return {{ tag: el.tagName.toLowerCase(), text: el.textContent.trim().substring(0, 100), index: {index}, total: els.length }};
            }})()"
        );
        self.eval(&js).await
    }

    /// Click the nth element matching a CSS selector (0-indexed).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn click_nth(&self, selector: &str, index: usize) -> Result<()> {
        let escaped = selector.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const els = document.querySelectorAll('{escaped}');
                if ({index} >= els.length) throw new Error('index {index} out of range, found ' + els.length + ' elements');
                els[{index}].click();
            }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }

    /// Highlight an element with a visible red border overlay.
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn highlight(&self, target: &str) -> Result<()> {
        let escaped = target.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            r"(() => {{
                const el = document.querySelector('{escaped}');
                if (!el) throw new Error('element not found: {escaped}');
                el.style.outline = '2px solid red';
                el.style.outlineOffset = '-1px';
            }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }
}
