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
//! page.click("@e1").await?;
//! # Ok(())
//! # }
//! ```
//!
//! # Module layout
//!
//! | Module | Purpose |
//! |--------|---------|
//! | [`page`] | `Page` struct + core (eval, dialog, logs) |
//! | [`resolve`] | Target resolution (ref/CSS → CDP objects) |
//! | [`navigate`] | Navigation + waiting |
//! | [`interact`] | Click, fill, type, hover, scroll, keyboard, mouse, touch |
//! | [`observe`] | Snapshot, screenshot, text/attribute queries, semantic locators |
//! | [`emulate`] | Viewport, media, offline, geolocation, timezone, locale |
//! | [`storage`] | Cookies + localStorage/sessionStorage |
//! | [`dom`] | Upload, drag, highlight, PDF, script/style injection |

// --- Core ---
mod browser;
mod config;
pub mod diff;
mod error;
pub mod page;
mod snapshot;

// --- Page capabilities (impl Page blocks) ---
mod dom;
mod emulate;
mod interact;
mod navigate;
mod observe;
mod resolve;
mod storage;

// --- Re-exports: core ---
pub use browser::Browser;
pub use config::{BrowserConfig, DEVICE_PRESETS, DevicePreset};
pub use error::{Error, Result};
pub use page::{ConsoleEntry, DialogInfo, JsError, Page};
pub use snapshot::{Ref, RefMap, Snapshot, SnapshotOptions};

// --- Re-exports: diffing ---
pub use diff::{ScreenshotDiff, SnapshotDiff, diff_rgba, diff_snapshots};

// --- Re-exports: domain types ---
pub use interact::{
    CdpKeyEventType, CdpMouseEventType, CdpTouchEventType, MouseButton, RawMouseEvent,
    ScrollDirection,
};
pub use observe::ImageFormat;
pub use storage::CookieInput;
