//! Element interaction: click, dblclick, fill, type, focus, hover, select, check, uncheck, scroll.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventType,
};

use crate::error::{Error, Result};

use super::{MouseButton, Page, ScrollDirection};

impl Page {
    /// Click an element by ref or CSS selector.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn click(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.click(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Double-click an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn dblclick(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        // Two rapid clicks via CDP Input domain
        for _ in 0..2 {
            self.dispatch_mouse(DispatchMouseEventType::MousePressed, center, 1)
                .await?;
            self.dispatch_mouse(DispatchMouseEventType::MouseReleased, center, 1)
                .await?;
        }
        Ok(())
    }

    /// Fill an input by clearing it first, then typing.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn fill(&self, target: &str, value: &str) -> Result<()> {
        self.focus(target).await?;
        self.key_press("Control+a").await?;
        self.key_press("Delete").await?;
        self.type_text(value).await
    }

    /// Type text into an already-focused element (or focus the target first).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn type_into(&self, target: &str, text: &str) -> Result<()> {
        self.focus(target).await?;
        self.type_text(text).await
    }

    /// Focus an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn focus(&self, target: &str) -> Result<()> {
        if let Some(r) = self.try_resolve_ref(target).await {
            return self.focus_ref_element(&r).await;
        }
        let el = self.find_element(target).await?;
        el.focus().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Hover over an element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn hover(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.move_mouse(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Select a single option in a `<select>` element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn select_option(&self, target: &str, value: &str) -> Result<()> {
        let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");
        self.call_on_target(
            target,
            &format!(
                "function() {{ \
                 for (const o of this.options) {{ \
                   if (o.value === '{escaped_val}' || o.textContent.trim() === '{escaped_val}') \
                     {{ o.selected = true; this.dispatchEvent(new Event('change')); return; }} \
                 }} }}"
            ),
        )
        .await?;
        Ok(())
    }

    /// Check a checkbox.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn check(&self, target: &str) -> Result<()> {
        self.call_on_target(target, "function() { if (!this.checked) this.click(); }")
            .await?;
        Ok(())
    }

    /// Uncheck a checkbox.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn uncheck(&self, target: &str) -> Result<()> {
        self.call_on_target(target, "function() { if (this.checked) this.click(); }")
            .await?;
        Ok(())
    }

    /// Scroll the page or a specific element.
    ///
    /// # Errors
    ///
    /// Returns an error if the scroll JS fails.
    pub async fn scroll(
        &self,
        direction: ScrollDirection,
        pixels: i64,
        selector: Option<&str>,
    ) -> Result<()> {
        let (dx, dy) = match direction {
            ScrollDirection::Down => (0, pixels),
            ScrollDirection::Up => (0, -pixels),
            ScrollDirection::Right => (pixels, 0),
            ScrollDirection::Left => (-pixels, 0),
        };
        if let Some(sel) = selector {
            let escaped = sel.replace('\\', "\\\\").replace('\'', "\\'");
            self.eval(&format!(
                "document.querySelector('{escaped}')?.scrollBy({dx},{dy})"
            ))
            .await?;
        } else {
            self.eval(&format!("window.scrollBy({dx},{dy})")).await?;
        }
        Ok(())
    }

    /// Click with specific button, click count, and optional delay between
    /// mouse-down and mouse-up (in milliseconds).
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn click_with(
        &self,
        target: &str,
        button: MouseButton,
        click_count: u32,
        delay_ms: u64,
    ) -> Result<()> {
        if button == MouseButton::Left && click_count == 1 && delay_ms == 0 {
            return self.click(target).await;
        }
        let center = self.resolve_target_center(target).await?;
        let cdp_btn = Self::to_cdp_button(button);
        let count = i64::from(click_count);
        self.dispatch_mouse_with(
            DispatchMouseEventType::MousePressed,
            center,
            count,
            cdp_btn.clone(),
        )
        .await?;
        if delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        self.dispatch_mouse_with(
            DispatchMouseEventType::MouseReleased,
            center,
            count,
            cdp_btn,
        )
        .await
    }

    /// Select multiple options on a `<select>` element.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
    pub async fn select_options(&self, target: &str, values: &[String]) -> Result<()> {
        for v in values {
            self.select_option(target, v).await?;
        }
        Ok(())
    }

    /// Type text with a per-character delay (0 = default behavior).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn type_with_delay(
        &self,
        target: Option<&str>,
        text: &str,
        delay_ms: u64,
    ) -> Result<()> {
        if let Some(t) = target {
            self.focus(t).await?;
        }
        if delay_ms == 0 {
            return self.type_text(text).await;
        }
        for ch in text.chars() {
            let s = ch.to_string();
            self.inner
                .execute(
                    DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::Char)
                        .text(s)
                        .build()
                        .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
                )
                .await
                .map_err(Error::Cdp)?;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        Ok(())
    }

    /// Set an input value directly via JS (no events fired).
    ///
    /// # Errors
    ///
    /// Returns an error if JS evaluation fails.
    pub async fn set_value(&self, target: &str, value: &str) -> Result<()> {
        let escaped_sel = target.replace('\'', "\\'");
        let escaped_val = value.replace('\\', "\\\\").replace('\'', "\\'");
        let js = format!(
            "(() => {{ const el = document.querySelector('{escaped_sel}'); \
             if (!el) throw new Error('Element not found: {escaped_sel}'); \
             el.value = '{escaped_val}'; }})()"
        );
        self.eval(&js).await?;
        Ok(())
    }
}
