#![allow(clippy::missing_errors_doc)]
//! # Brother
//!
//! Browser automation for AI agents, built on Chrome `DevTools` Protocol.
//!
//! Brother provides a high-level Rust API for headless browser control,
//! with first-class support for accessibility snapshots and ref-based
//! element interaction — the optimal workflow for LLM-driven agents.
//!
//! # Quick Start
//!
//! ```no_run
//! use brother::{Browser, BrowserConfig};
//! use futures::StreamExt;
//!
//! # async fn example() -> brother::Result<()> {
//! let (browser, mut handler) = Browser::launch(BrowserConfig::default()).await?;
//! tokio::spawn(async move { while handler.next().await.is_some() {} });
//!
//! let page = browser.new_page("https://example.com").await?;
//! let snapshot = page.snapshot().await?;
//! println!("{}", snapshot.tree());
//!
//! // Click element by ref from snapshot
//! page.click("@e1").await?;
//! # Ok(())
//! # }
//! ```

mod browser;
mod config;
pub mod diff;
mod error;
mod page;
mod snapshot;

pub use browser::Browser;
pub use config::{BrowserConfig, DEVICE_PRESETS, DevicePreset};
pub use diff::{ScreenshotDiff, SnapshotDiff, diff_rgba, diff_snapshots};
pub use error::{Error, Result};
pub use page::{
    CdpKeyEventType, CdpMouseEventType, CdpTouchEventType, ConsoleEntry, CookieInput, DialogInfo,
    ImageFormat, JsError, MouseButton, Page, RawMouseEvent, ScrollDirection,
};
pub use snapshot::{Ref, RefMap, Snapshot, SnapshotOptions};
