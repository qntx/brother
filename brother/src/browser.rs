//! Browser lifecycle management.

use chromiumoxide::handler::Handler;

use crate::config::BrowserConfig;
use crate::error::{Error, Result};
use crate::page::Page;

/// A browser instance connected via Chrome `DevTools` Protocol.
///
/// The browser launches a Chrome/Chromium process and communicates over CDP.
/// Use [`Browser::launch`] to start a new instance, or [`Browser::connect`]
/// to attach to an existing browser via a CDP `WebSocket` URL.
#[derive(Debug)]
pub struct Browser {
    inner: chromiumoxide::Browser,
}

impl Browser {
    /// Launch a new browser process with the given configuration.
    ///
    /// Returns the browser instance and a [`Handler`] that must be spawned
    /// as a background task to process CDP events.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn example() -> brother::Result<()> {
    /// let (browser, mut handler) = brother::Browser::launch(
    ///     brother::BrowserConfig::default(),
    /// ).await?;
    /// tokio::spawn(async move { while handler.next().await.is_some() {} });
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if Chrome cannot be found or launched.
    pub async fn launch(config: BrowserConfig) -> Result<(Self, Handler)> {
        let chromium_config = config.into_chromium_config()?;

        let (browser, handler) = chromiumoxide::Browser::launch(chromium_config)
            .await
            .map_err(|e| Error::Browser(format!("failed to launch browser: {e}")))?;

        tracing::info!("browser launched");

        Ok((Self { inner: browser }, handler))
    }

    /// Connect to an existing browser via CDP `WebSocket` URL.
    ///
    /// # Example
    ///
    /// ```no_run
    /// # use futures::StreamExt;
    /// # async fn example() -> brother::Result<()> {
    /// let (browser, mut handler) = brother::Browser::connect(
    ///     "ws://127.0.0.1:9222/devtools/browser/..."
    /// ).await?;
    /// tokio::spawn(async move { while handler.next().await.is_some() {} });
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the connection fails.
    pub async fn connect(ws_url: &str) -> Result<(Self, Handler)> {
        let (browser, handler) = chromiumoxide::Browser::connect(ws_url)
            .await
            .map_err(|e| Error::Browser(format!("failed to connect: {e}")))?;

        tracing::info!(url = ws_url, "connected to browser");

        Ok((Self { inner: browser }, handler))
    }

    /// Open a new page (tab) and navigate to the given URL.
    ///
    /// # Errors
    ///
    /// Returns an error if page creation or navigation fails.
    pub async fn new_page(&self, url: &str) -> Result<Page> {
        let page = self
            .inner
            .new_page(url)
            .await
            .map_err(|e| Error::Navigation(format!("failed to open page: {e}")))?;

        tracing::debug!(url, "new page opened");

        Ok(Page::new(page))
    }

    /// Open a blank new page (tab).
    ///
    /// # Errors
    ///
    /// Returns an error if page creation fails.
    pub async fn new_blank_page(&self) -> Result<Page> {
        self.new_page("about:blank").await
    }

    /// Get all open pages (tabs).
    ///
    /// # Errors
    ///
    /// Returns an error if the CDP query fails.
    pub async fn pages(&self) -> Result<Vec<Page>> {
        let pages = self.inner.pages().await.map_err(Error::Cdp)?;
        Ok(pages.into_iter().map(Page::new).collect())
    }

    /// Close the browser and kill the Chrome process.
    ///
    /// # Errors
    ///
    /// Returns an error if the close command fails.
    pub async fn close(mut self) -> Result<()> {
        self.inner.close().await.map_err(Error::Cdp)?;
        tracing::info!("browser closed");
        Ok(())
    }

    /// Get a reference to the underlying `chromiumoxide::Browser`.
    ///
    /// Escape hatch for advanced CDP operations not covered by this API.
    #[must_use]
    pub const fn inner(&self) -> &chromiumoxide::Browser {
        &self.inner
    }
}
