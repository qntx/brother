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
mod error;
mod page;
mod snapshot;

pub use browser::Browser;
pub use config::{BrowserConfig, DevicePreset, DEVICE_PRESETS};
pub use error::{Error, Result};
pub use page::{
    ConsoleEntry, CookieInput, DialogInfo, JsError, MouseButton, Page, ScrollDirection,
};
pub use snapshot::{Ref, RefMap, Snapshot, SnapshotOptions};
