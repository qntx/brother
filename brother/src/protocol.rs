//! JSON protocol for daemon ↔ CLI communication.
//!
//! The daemon listens on `127.0.0.1:<port>`. Each message is a single JSON
//! object terminated by `\n`. The CLI sends a [`Request`]; the daemon replies
//! with a [`Response`].

use serde::{Deserialize, Serialize};

use crate::snapshot::SnapshotOptions;

// ---------------------------------------------------------------------------
// Request  (CLI → Daemon)
// ---------------------------------------------------------------------------

/// A command sent from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    // -- Navigation --------------------------------------------------------
    /// Navigate the active page to a URL.
    Navigate {
        /// Target URL.
        url: String,
        /// Wait strategy after navigation.
        #[serde(default)]
        wait: WaitStrategy,
    },
    /// Go back in history.
    Back,
    /// Go forward in history.
    Forward,
    /// Reload the current page.
    Reload,

    // -- Observation -------------------------------------------------------
    /// Capture an accessibility snapshot.
    Snapshot {
        /// Snapshot filtering options.
        #[serde(default)]
        options: SnapshotOptions,
    },
    /// Take a screenshot (base64-encoded PNG).
    Screenshot {
        /// Capture the full scrollable page.
        #[serde(default)]
        full_page: bool,
    },
    /// Evaluate `JavaScript` and return the result.
    Eval {
        /// JS expression.
        expression: String,
    },

    // -- Interaction (target = ref `@e1` or CSS selector) ------------------
    /// Click an element.
    Click {
        /// Ref or CSS selector.
        target: String,
    },
    /// Double-click an element.
    DblClick {
        /// Ref or CSS selector.
        target: String,
    },
    /// Clear and fill an input.
    Fill {
        /// Ref or CSS selector.
        target: String,
        /// Value to fill.
        value: String,
    },
    /// Type text into the focused element (or a target).
    Type {
        /// Optional ref or CSS selector; types into focused element if absent.
        target: Option<String>,
        /// Text to type.
        text: String,
    },
    /// Press a key combo (e.g. `"Enter"`, `"Control+a"`).
    Press {
        /// Key or key combo.
        key: String,
    },
    /// Select a dropdown option by value.
    Select {
        /// Ref or CSS selector of the `<select>` element.
        target: String,
        /// Option value to select.
        value: String,
    },
    /// Check a checkbox (no-op if already checked).
    Check {
        /// Ref or CSS selector.
        target: String,
    },
    /// Uncheck a checkbox (no-op if already unchecked).
    Uncheck {
        /// Ref or CSS selector.
        target: String,
    },
    /// Hover an element.
    Hover {
        /// Ref or CSS selector.
        target: String,
    },
    /// Focus an element.
    Focus {
        /// Ref or CSS selector.
        target: String,
    },
    /// Scroll the page or an element.
    Scroll {
        /// Direction to scroll.
        direction: ScrollDirection,
        /// Pixels to scroll (default 500).
        #[serde(default = "default_scroll_px")]
        pixels: i64,
        /// Optional target to scroll (defaults to viewport).
        #[serde(default)]
        target: Option<String>,
    },

    // -- Query -------------------------------------------------------------
    /// Get text content (whole page or scoped by target).
    GetText {
        /// Optional ref or CSS selector.
        #[serde(default)]
        target: Option<String>,
    },
    /// Get the current page URL.
    GetUrl,
    /// Get the current page title.
    GetTitle,
    /// Get `innerHTML` of an element.
    GetHtml {
        /// Ref or CSS selector.
        target: String,
    },
    /// Get the `value` property of an input element.
    GetValue {
        /// Ref or CSS selector.
        target: String,
    },
    /// Get an attribute of an element.
    GetAttribute {
        /// Ref or CSS selector.
        target: String,
        /// Attribute name.
        attribute: String,
    },

    // -- Wait --------------------------------------------------------------
    /// Wait for a condition.
    Wait {
        /// What to wait for.
        condition: WaitCondition,
    },

    // -- Lifecycle ---------------------------------------------------------
    /// Check daemon health / browser status.
    Status,
    /// Close the browser and shut down the daemon.
    Close,
}

// ---------------------------------------------------------------------------
// Scroll
// ---------------------------------------------------------------------------

/// Direction for the `Scroll` command.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScrollDirection {
    /// Scroll down (positive Y).
    Down,
    /// Scroll up (negative Y).
    Up,
    /// Scroll right (positive X).
    Right,
    /// Scroll left (negative X).
    Left,
}

/// Default scroll distance in pixels.
const fn default_scroll_px() -> i64 {
    500
}

// ---------------------------------------------------------------------------
// Wait
// ---------------------------------------------------------------------------

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
const fn default_timeout_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// Response  (Daemon → CLI)
// ---------------------------------------------------------------------------

/// Response from the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    /// Command succeeded.
    Ok {
        /// Optional payload.
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<ResponseData>,
    },
    /// Command failed.
    Error {
        /// Human-readable error message.
        message: String,
    },
}

impl Response {
    /// Success with no payload.
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok { data: None }
    }

    /// Success with payload.
    #[must_use]
    pub const fn ok_data(data: ResponseData) -> Self {
        Self::Ok { data: Some(data) }
    }

    /// Error response.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

/// Payload variants returned by commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResponseData {
    /// Navigation result.
    Navigate {
        /// Final URL after navigation.
        url: String,
        /// Page title.
        title: String,
    },
    /// Accessibility snapshot.
    Snapshot {
        /// Formatted tree text.
        tree: String,
        /// Ref metadata map.
        refs: serde_json::Value,
    },
    /// Screenshot (base64 PNG).
    Screenshot {
        /// Base64-encoded image data.
        data: String,
    },
    /// `JavaScript` evaluation result.
    Eval {
        /// Serialized result.
        value: serde_json::Value,
    },
    /// Generic text result (url, title, text, html, value, attribute).
    Text {
        /// The text content.
        text: String,
    },
    /// Daemon / browser status.
    Status {
        /// Whether a browser is running.
        browser_running: bool,
        /// Current page URL (if any).
        page_url: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Runtime directory helpers
// ---------------------------------------------------------------------------

/// Runtime directory for daemon files (`~/.brother/`).
#[must_use]
pub fn runtime_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("brother"))
}

/// Path to the daemon port file.
#[must_use]
pub fn port_file_path() -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join("daemon.port"))
}

/// Path to the daemon PID file.
#[must_use]
pub fn pid_file_path() -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join("daemon.pid"))
}
