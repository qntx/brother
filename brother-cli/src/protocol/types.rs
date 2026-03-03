//! Shared types used by the request/response protocol.

use serde::{Deserialize, Serialize};

/// Action for network route interception.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RouteAction {
    /// Respond with a custom body/status.
    #[default]
    Fulfill,
    /// Block the request.
    Abort,
}

/// Strategy to wait after navigation.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitStrategy {
    /// Wait for `load` event (default).
    #[default]
    Load,
    /// Wait for `DOMContentLoaded` event.
    DomContentLoaded,
    /// Wait until no in-flight network requests for 500 ms.
    NetworkIdle,
}

/// Condition the `Wait` command blocks on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WaitCondition {
    /// Wait for a CSS selector to appear.
    Selector {
        /// CSS selector.
        selector: String,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for text to appear on the page.
    Text {
        /// Substring to look for.
        text: String,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for the URL to contain a pattern.
    Url {
        /// Substring or pattern.
        pattern: String,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for a JS expression to return truthy.
    Function {
        /// JS expression.
        expression: String,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for a load state.
    LoadState {
        /// Which load state.
        state: WaitStrategy,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for a fixed duration.
    Duration {
        /// Milliseconds to sleep.
        ms: u64,
    },
}

/// Default timeout: 30 s.
pub const fn default_timeout_ms() -> u64 {
    30_000
}

pub const fn default_viewport_width() -> u32 {
    1280
}

pub const fn default_viewport_height() -> u32 {
    720
}

pub const fn default_diff_threshold() -> u8 {
    10
}

pub const fn default_scroll_px() -> i64 {
    500
}

pub const fn default_status() -> u16 {
    200
}

pub fn default_content_type() -> String {
    "text/plain".into()
}

pub const fn default_geo_accuracy() -> f64 {
    1.0
}

pub const fn default_true() -> bool {
    true
}

pub const fn default_click_count() -> u32 {
    1
}

pub fn default_screenshot_format() -> String {
    "png".into()
}

pub const fn default_jpeg_quality() -> u8 {
    80
}

pub fn default_screencast_format() -> String {
    "jpeg".into()
}

pub const fn default_screencast_quality() -> u32 {
    80
}

