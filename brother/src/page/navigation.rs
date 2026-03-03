//! Navigation methods: goto, back, forward, reload.

use chromiumoxide::cdp::browser_protocol::page::NavigateToHistoryEntryParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Navigate to a URL and wait for the page to load.
    ///
    /// # Errors
    ///
    /// Returns an error if navigation fails or the URL is invalid.
    pub async fn goto(&self, url: &str) -> Result<()> {
        self.inner.goto(url).await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go back in history.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn go_back(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        #[allow(clippy::cast_possible_wrap)]
        let entry_id = idx.saturating_sub(1) as i64;
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Go forward in history.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn go_forward(&self) -> Result<()> {
        let idx = self.current_history_index().await?;
        #[allow(clippy::cast_possible_wrap)]
        let entry_id = (idx + 1) as i64;
        self.inner
            .execute(NavigateToHistoryEntryParams::new(entry_id))
            .await
            .map_err(Error::Cdp)?;
        Ok(())
    }

    /// Reload the current page.
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP command fails.
    pub async fn reload(&self) -> Result<()> {
        self.inner.reload().await.map_err(Error::Cdp)?;
        Ok(())
    }

    /// Wait for navigation to complete (network idle heuristic).
    ///
    /// # Errors
    ///
    /// Returns an error on timeout.
    pub async fn wait_for_navigation(&self) -> Result<()> {
        self.inner.wait_for_navigation().await.map_err(Error::Cdp)?;
        Ok(())
    }
}
