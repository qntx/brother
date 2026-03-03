//! Screenshot capture methods.

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;

use crate::error::{Error, Result};

use super::Page;

impl Page {
    /// Capture a PNG screenshot of the viewport.
    pub async fn screenshot_png(&self) -> Result<Vec<u8>> {
        self.inner
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Png)
                    .build(),
            )
            .await
            .map_err(Error::Cdp)
    }

    /// Capture a JPEG screenshot.
    pub async fn screenshot_jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        self.inner
            .screenshot(
                ScreenshotParams::builder()
                    .format(CaptureScreenshotFormat::Jpeg)
                    .quality(i64::from(quality))
                    .build(),
            )
            .await
            .map_err(Error::Cdp)
    }

    /// Capture a screenshot with full options (format, quality, selector, full page).
    pub async fn screenshot(
        &self,
        full_page: bool,
        selector: Option<&str>,
        format: &str,
        quality: Option<u8>,
    ) -> Result<Vec<u8>> {
        use chromiumoxide::cdp::browser_protocol::page::{
            CaptureScreenshotParams, Viewport as CdpViewport,
        };

        // If a selector is given, capture just that element's bounding box.
        if let Some(sel) = selector {
            let (x, y, w, h) = self.bounding_box(sel).await?;
            let clip = CdpViewport {
                x,
                y,
                width: w,
                height: h,
                scale: 1.0,
            };
            let fmt = if format == "jpeg" {
                CaptureScreenshotFormat::Jpeg
            } else {
                CaptureScreenshotFormat::Png
            };
            let mut params = CaptureScreenshotParams::builder().format(fmt).clip(clip);
            if let Some(q) = quality {
                params = params.quality(i64::from(q));
            }
            let data = self
                .inner
                .execute(params.build())
                .await
                .map_err(Error::Cdp)?;
            return base64::engine::general_purpose::STANDARD
                .decode(&data.result.data)
                .map_err(|e| Error::Browser(format!("base64 decode: {e}")));
        }

        // Full page or viewport screenshot.
        let fmt = if format == "jpeg" {
            CaptureScreenshotFormat::Jpeg
        } else {
            CaptureScreenshotFormat::Png
        };
        let mut builder = ScreenshotParams::builder().format(fmt);
        if full_page {
            builder = builder.full_page(true);
        }
        if let Some(q) = quality {
            builder = builder.quality(i64::from(q));
        }
        self.inner
            .screenshot(builder.build())
            .await
            .map_err(Error::Cdp)
    }
}
