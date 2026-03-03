//! Clipboard read/write methods.

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Read text from the clipboard (grants permission first).
    ///
    /// # Errors
    ///
    /// Returns an error if permission or JS evaluation fails.
    pub async fn clipboard_read(&self) -> Result<String> {
        self.grant_clipboard_permission().await?;
        let val = self.eval("navigator.clipboard.readText()").await?;
        Ok(val.as_str().unwrap_or("").to_owned())
    }

    /// Write text to the clipboard (grants permission first).
    ///
    /// # Errors
    ///
    /// Returns an error if permission or JS evaluation fails.
    pub async fn clipboard_write(&self, text: &str) -> Result<()> {
        self.grant_clipboard_permission().await?;
        let escaped = text.replace('\\', "\\\\").replace('\'', "\\'");
        self.eval(&format!("navigator.clipboard.writeText('{escaped}')"))
            .await?;
        Ok(())
    }

    /// Grant clipboard-read and clipboard-write permissions.
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
