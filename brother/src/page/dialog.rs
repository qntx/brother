//! Dialog handling methods (alert, confirm, prompt).

use crate::error::{Error, Result};

use super::{DialogInfo, Page};

impl Page {
    /// Get the most recent dialog info (if a dialog is open).
    pub async fn dialog_message(&self) -> Option<DialogInfo> {
        self.dialog.lock().await.clone()
    }

    /// Accept (OK) the current `JavaScript` dialog, optionally providing
    /// prompt text.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn dialog_accept(&self, prompt_text: Option<&str>) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
        let mut params = HandleJavaScriptDialogParams::new(true);
        if let Some(text) = prompt_text {
            params.prompt_text = Some(text.to_owned());
        }
        self.inner.execute(params).await.map_err(Error::Cdp)?;
        *self.dialog.lock().await = None;
        Ok(())
    }

    /// Dismiss (Cancel) the current `JavaScript` dialog.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn dialog_dismiss(&self) -> Result<()> {
        use chromiumoxide::cdp::browser_protocol::page::HandleJavaScriptDialogParams;
        self.inner
            .execute(HandleJavaScriptDialogParams::new(false))
            .await
            .map_err(Error::Cdp)?;
        *self.dialog.lock().await = None;
        Ok(())
    }
}
