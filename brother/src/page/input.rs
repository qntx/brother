//! Keyboard, mouse, and touch input methods.

use std::time::Duration;

use chromiumoxide::cdp::browser_protocol::input::{
    DispatchKeyEventParams, DispatchKeyEventType, DispatchMouseEventParams, DispatchMouseEventType,
};

use crate::error::{Error, Result};

use super::{MouseButton, Page};

impl Page {
    /// Press a key or combo (e.g. `"Enter"`, `"Tab"`, `"Control+a"`,
    /// `"Shift+Control+ArrowUp"`).
    ///
    /// Modifier keys (`Control`, `Shift`, `Alt`, `Meta`) are held down
    /// while the final key is pressed and released, then released in
    /// reverse order.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn key_press(&self, key: &str) -> Result<()> {
        let parts: Vec<&str> = key.split('+').collect();
        if parts.len() == 1 {
            self.dispatch_key(DispatchKeyEventType::KeyDown, key)
                .await?;
            return self.dispatch_key(DispatchKeyEventType::KeyUp, key).await;
        }
        // Hold modifiers, press final key, release in reverse
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

    /// Type text character by character.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn type_text(&self, text: &str) -> Result<()> {
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
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    /// Press and hold a key (without releasing).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn key_down(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyDown, key).await
    }

    /// Release a held key.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn key_up(&self, key: &str) -> Result<()> {
        self.dispatch_key(DispatchKeyEventType::KeyUp, key).await
    }

    /// Insert text directly without firing individual key events.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP call fails.
    pub async fn insert_text(&self, text: &str) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::input::InsertTextParams;
        let params = InsertTextParams::new(text.to_owned());
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Mouse / Touch
    // -----------------------------------------------------------------------

    /// Move the mouse to absolute coordinates.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
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

    /// Touch-tap an element by resolving its center and dispatching touch events.
    ///
    /// # Errors
    ///
    /// Returns an error if the element is not found.
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
}
