//! Screenshot capture methods.

use base64::Engine;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotFormat;
use chromiumoxide::page::ScreenshotParams;

use crate::error::{Error, Result};

use super::Page;

/// Image format for screenshots and screencasts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageFormat {
    /// PNG (lossless, default).
    #[default]
    Png,
    /// JPEG (lossy, smaller file size).
    Jpeg,
}

impl ImageFormat {
    /// File extension for this format.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    const fn to_cdp(self) -> CaptureScreenshotFormat {
        match self {
            Self::Png => CaptureScreenshotFormat::Png,
            Self::Jpeg => CaptureScreenshotFormat::Jpeg,
        }
    }

    /// Parse from a string (case-insensitive). Returns `Png` for unknown values.
    #[must_use]
    pub const fn from_str_lossy(s: &str) -> Self {
        if s.eq_ignore_ascii_case("jpeg") || s.eq_ignore_ascii_case("jpg") {
            Self::Jpeg
        } else {
            Self::Png
        }
    }
}

impl std::fmt::Display for ImageFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Png => f.write_str("png"),
            Self::Jpeg => f.write_str("jpeg"),
        }
    }
}

impl std::str::FromStr for ImageFormat {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            other => Err(format!("unknown image format '{other}', expected 'png' or 'jpeg'")),
        }
    }
}

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
        format: ImageFormat,
        quality: Option<u8>,
    ) -> Result<Vec<u8>> {
        use chromiumoxide::cdp::browser_protocol::page::{
            CaptureScreenshotParams, Viewport as CdpViewport,
        };

        let fmt = format.to_cdp();

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
