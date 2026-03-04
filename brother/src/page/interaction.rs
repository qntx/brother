//! Element interaction, keyboard/mouse/touch input, and clipboard.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
    MouseButton as CdpMouseButton,
};

use crate::error::{Error, Result};

use super::{
    CdpKeyEventType, CdpMouseEventType, CdpTouchEventType, MouseButton, Page, ScrollDirection,
};

/// Raw CDP mouse event for injection (pair browsing / stream server).
#[derive(Debug)]
pub struct RawMouseEvent {
    /// Event type.
    pub event_type: CdpMouseEventType,
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
    /// Button: `"left"`, `"right"`, `"middle"`, `"none"`.
    pub button: Option<String>,
    /// Click count.
    pub click_count: Option<i64>,
    /// Wheel delta X.
    pub delta_x: Option<f64>,
    /// Wheel delta Y.
    pub delta_y: Option<f64>,
    /// Modifier flags (1=Alt, 2=Ctrl, 4=Meta, 8=Shift).
    pub modifiers: Option<i64>,
}

impl Page {
    /// Click an element by ref or CSS selector.
    pub async fn click(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.click(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Double-click an element.
    pub async fn dblclick(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        // First click (click_count = 1)
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, center, 1)
            .await?;
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, center, 1)
            .await?;
        // Second click (click_count = 2) — triggers the browser dblclick event
        self.dispatch_mouse(DispatchMouseEventType::MousePressed, center, 2)
            .await?;
        self.dispatch_mouse(DispatchMouseEventType::MouseReleased, center, 2)
            .await?;
        Ok(())
    }

    /// Fill an input by clearing it first, then typing.
    pub async fn fill(&self, target: &str, value: &str) -> Result<()> {
        self.focus(target).await?;
        self.key_press("Control+a").await?;
        self.key_press("Delete").await?;
        self.type_text(value).await
    }

    /// Type text into an already-focused element (or focus the target first).
    pub async fn type_into(&self, target: &str, text: &str) -> Result<()> {
        self.focus(target).await?;
        self.type_text(text).await
    }

    /// Focus an element.
    pub async fn focus(&self, target: &str) -> Result<()> {
        if let Some(r) = self.try_resolve_ref(target).await {
            return self.focus_ref_element(&r).await;
        }
        let el = self.find_element(target).await?;
        el.focus().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Hover over an element.
    pub async fn hover(&self, target: &str) -> Result<()> {
        let center = self.resolve_target_center(target).await?;
        self.inner.move_mouse(center).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Select a single option in a `<select>` element.
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

    /// Check a checkbox (scrolls into view first).
    pub async fn check(&self, target: &str) -> Result<()> {
        self.call_on_target(
            target,
            "function() { this.scrollIntoView({block:'center'}); if (!this.checked) this.click(); }",
        )
        .await?;
        Ok(())
    }

    /// Uncheck a checkbox (scrolls into view first).
    pub async fn uncheck(&self, target: &str) -> Result<()> {
        self.call_on_target(
            target,
            "function() { this.scrollIntoView({block:'center'}); if (this.checked) this.click(); }",
        )
        .await?;
        Ok(())
    }

    /// Scroll the page or a specific element.
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

    /// Click with specific button, click count, and optional delay.
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
    pub async fn select_options(&self, target: &str, values: &[String]) -> Result<()> {
        for v in values {
            self.select_option(target, v).await?;
        }
        Ok(())
    }

    /// Type text with a per-character delay (0 = default 10 ms).
    pub async fn type_with_delay(
        &self,
        target: Option<&str>,
        text: &str,
        delay_ms: u64,
    ) -> Result<()> {
        if let Some(t) = target {
            self.focus(t).await?;
        }
        let delay = if delay_ms == 0 { 10 } else { delay_ms };
        self.type_chars(text, delay).await
    }

    /// Set an input value directly via JS (no key events fired).
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

    /// Press a key or combo (e.g. `"Enter"`, `"Tab"`, `"Control+a"`).
    ///
    /// Modifier keys are held down while the final key is pressed/released,
    /// then released in reverse order.
    pub async fn key_press(&self, key: &str) -> Result<()> {
        let parts: Vec<&str> = key.split('+').collect();
        if parts.len() == 1 {
            self.dispatch_key(DispatchKeyEventType::KeyDown, key)
                .await?;
            return self.dispatch_key(DispatchKeyEventType::KeyUp, key).await;
        }
        let (modifiers, final_key) = parts.split_at(parts.len() - 1);
        for &m in modifiers {
            self.dispatch_key(DispatchKeyEventType::KeyDown, m).await?;
        }
        self.dispatch_key(DispatchKeyEventType::KeyDown, final_key[0])
            .await?;
        self.dispatch_key(DispatchKeyEventType::KeyUp, final_key[0])
            .await?;
        for &m in modifiers.iter().rev() {
            self.dispatch_key(DispatchKeyEventType::KeyUp, m).await?;
        }
        Ok(())
    }

    /// Type text character by character (10 ms inter-key delay).
    pub async fn type_text(&self, text: &str) -> Result<()> {
        self.type_chars(text, 10).await
    }

    /// Dispatch each character as a `Char` key event with `delay_ms` between keystrokes.
    async fn type_chars(&self, text: &str, delay_ms: u64) -> Result<()> {
        for ch in text.chars() {
            self.inner
                .execute(
                    DispatchKeyEventParams::builder()
                        .r#type(DispatchKeyEventType::Char)
                        .text(ch.to_string())
                        .build()
                        .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?,
                )
                .await
                .map_err(Error::Cdp)?;
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        }
        Ok(())
    }

    /// Press and hold a key (without releasing).
    pub async fn key_down(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyDown, key).await
    }

    /// Release a held key.
    pub async fn key_up(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyUp, key).await
    }

    /// Insert text directly without firing individual key events.
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
        self.inner
            .execute(InsertTextParams::new(text.to_owned()))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Move the mouse to absolute coordinates.
    pub async fn mouse_move(&self, x: f64, y: f64) -> Result<()> {
        let params = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseMoved)
            .x(x)
            .y(y)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Press a mouse button down at the current position.
    pub async fn mouse_down(&self, button: MouseButton) -> Result<()> {
        let params = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MousePressed)
            .button(Self::to_cdp_button(button))
            .x(0)
            .y(0)
            .click_count(1)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Release a mouse button at the current position.
    pub async fn mouse_up(&self, button: MouseButton) -> Result<()> {
        let params = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseReleased)
            .button(Self::to_cdp_button(button))
            .x(0)
            .y(0)
            .click_count(1)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Scroll with the mouse wheel, optionally targeting an element.
    pub async fn wheel(&self, delta_x: f64, delta_y: f64, selector: Option<&str>) -> Result<()> {
        if let Some(sel) = selector {
            self.hover(sel).await?;
        }
        let params = DispatchMouseEventParams::builder()
            .r#type(DispatchMouseEventType::MouseWheel)
            .x(0)
            .y(0)
            .delta_x(delta_x)
            .delta_y(delta_y)
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Touch-tap an element.
    pub async fn tap(&self, target: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchTouchEventParams, DispatchTouchEventType, TouchPoint,
        };
        let center = self.resolve_target_center(target).await?;
        let point = TouchPoint::new(center.x, center.y);
        let start =
            DispatchTouchEventParams::new(DispatchTouchEventType::TouchStart, vec![point.clone()]);
        self.inner.execute(start).await.map_err(Error::Cdp)?;
        let end = DispatchTouchEventParams::new(DispatchTouchEventType::TouchEnd, vec![]);
        self.inner.execute(end).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Inject a raw CDP mouse event (for stream server / pair browsing).
    pub async fn inject_mouse_event(&self, ev: RawMouseEvent) -> Result<()> {
        let cdp_type = match ev.event_type {
            CdpMouseEventType::MousePressed => DispatchMouseEventType::MousePressed,
            CdpMouseEventType::MouseReleased => DispatchMouseEventType::MouseReleased,
            CdpMouseEventType::MouseMoved => DispatchMouseEventType::MouseMoved,
            CdpMouseEventType::MouseWheel => DispatchMouseEventType::MouseWheel,
        };
        let cdp_button = match ev.button.as_deref().unwrap_or("none") {
            "left" => CdpMouseButton::Left,
            "right" => CdpMouseButton::Right,
            "middle" => CdpMouseButton::Middle,
            _ => CdpMouseButton::None,
        };
        let mut builder = DispatchMouseEventParams::builder()
            .r#type(cdp_type)
            .x(ev.x)
            .y(ev.y)
            .button(cdp_button);
        if let Some(cc) = ev.click_count {
            builder = builder.click_count(cc);
        }
        if let Some(dx) = ev.delta_x {
            builder = builder.delta_x(dx);
        }
        if let Some(dy) = ev.delta_y {
            builder = builder.delta_y(dy);
        }
        if let Some(m) = ev.modifiers {
            builder = builder.modifiers(m);
        }
        let params = builder
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Inject a raw CDP keyboard event (for stream server / pair browsing).
    pub async fn inject_key_event(
        &self,
        event_type: CdpKeyEventType,
        key: Option<&str>,
        code: Option<&str>,
        text: Option<&str>,
        modifiers: Option<i64>,
    ) -> Result<()> {
        let cdp_type = match event_type {
            CdpKeyEventType::KeyDown => DispatchKeyEventType::KeyDown,
            CdpKeyEventType::KeyUp => DispatchKeyEventType::KeyUp,
            CdpKeyEventType::Char => DispatchKeyEventType::Char,
        };
        let mut builder = DispatchKeyEventParams::builder().r#type(cdp_type);
        if let Some(k) = key {
            builder = builder.key(k);
        }
        if let Some(c) = code {
            builder = builder.code(c);
        }
        if let Some(t) = text {
            builder = builder.text(t);
        }
        if let Some(m) = modifiers {
            builder = builder.modifiers(m);
        }
        let params = builder
            .build()
            .map_err(|e| Error::Cdp(chromiumoxide::error::CdpError::msg(e)))?;
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Inject a raw CDP touch event (for stream server / pair browsing).
    pub async fn inject_touch_event(
        &self,
        event_type: CdpTouchEventType,
        touch_points: &[(f64, f64)],
        modifiers: Option<i64>,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchTouchEventParams, DispatchTouchEventType, TouchPoint,
        };
        let cdp_type = match event_type {
            CdpTouchEventType::TouchStart => DispatchTouchEventType::TouchStart,
            CdpTouchEventType::TouchEnd => DispatchTouchEventType::TouchEnd,
            CdpTouchEventType::TouchMove => DispatchTouchEventType::TouchMove,
            CdpTouchEventType::TouchCancel => DispatchTouchEventType::TouchCancel,
        };
        let points: Vec<TouchPoint> = touch_points
            .iter()
            .map(|&(x, y)| TouchPoint::new(x, y))
            .collect();
        let mut params = DispatchTouchEventParams::new(cdp_type, points);
        if let Some(m) = modifiers {
            params.modifiers = Some(m);
        }
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Perform a swipe gesture on an element.
    ///
    /// Resolves the element center, then dispatches a touch event sequence:
    /// `touchStart` at center → several `touchMove` steps → `touchEnd` at final position.
    pub async fn swipe(
        &self,
        target: &str,
        direction: ScrollDirection,
        distance: i64,
    ) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::{
            DispatchTouchEventParams, DispatchTouchEventType, TouchPoint,
        };

        let center = self.resolve_target_center(target).await?;
        let (cx, cy) = (center.x, center.y);
        let (dx, dy): (f64, f64) = match direction {
            ScrollDirection::Up => (0.0, -(distance as f64)),
            ScrollDirection::Down => (0.0, distance as f64),
            ScrollDirection::Left => (-(distance as f64), 0.0),
            ScrollDirection::Right => (distance as f64, 0.0),
        };

        let steps = 10u32;
        let step_delay = Duration::from_millis(16);

        let start = DispatchTouchEventParams::new(
            DispatchTouchEventType::TouchStart,
            vec![TouchPoint::new(cx, cy)],
        );
        self.inner.execute(start).await.map_err(Error::Cdp)?;

        for i in 1..=steps {
            let frac = f64::from(i) / f64::from(steps);
            let px = dx.mul_add(frac, cx);
            let py = dy.mul_add(frac, cy);
            let mv = DispatchTouchEventParams::new(
                DispatchTouchEventType::TouchMove,
                vec![TouchPoint::new(px, py)],
            );
            self.inner.execute(mv).await.map_err(Error::Cdp)?;
            tokio::time::sleep(step_delay).await;
        }

        let end = DispatchTouchEventParams::new(
            DispatchTouchEventType::TouchEnd,
            vec![],
        );
        self.inner.execute(end).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Read text from the clipboard (grants permission first).
    pub async fn clipboard_read(&self) -> Result<String> {
        self.grant_clipboard_permission().await?;
        let val = self.eval("navigator.clipboard.readText()").await?;
        Ok(val.as_str().unwrap_or("").to_owned())
    }

    /// Write text to the clipboard (grants permission first).
    pub async fn clipboard_write(&self, text: &str) -> Result<()> {
        self.grant_clipboard_permission().await?;
        let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("navigator.clipboard.writeText('{escaped}')"))
            .await?;
        Ok(())
    }

    async fn grant_clipboard_permission(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::browser::{
            PermissionDescriptor, PermissionSetting, SetPermissionParams,
        };
        for perm_name in ["clipboard-read", "clipboard-write"] {
            let descriptor = PermissionDescriptor::new(perm_name.to_owned());
            let params = SetPermissionParams::new(descriptor, PermissionSetting::Granted);
            self.inner.execute(params).await.map_err(Error::Cdp)?;
        }
        Ok(())
    }
}
