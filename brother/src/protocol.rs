//! JSON protocol for daemon ↔ CLI communication.
//!
//! The daemon listens on a TCP socket (`127.0.0.1:<port>`). Each message is a
//! single JSON object terminated by a newline (`\n`). The CLI sends a
//! [`Request`] and the daemon replies with a [`Response`].

use serde::{Deserialize, Serialize};

use crate::snapshot::SnapshotOptions;

// ---------------------------------------------------------------------------
// Requests (CLI → Daemon)
// ---------------------------------------------------------------------------

/// A command sent from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Launch a new browser (no-op if already running).
    Launch {
        /// Whether to show the browser window.
        #[serde(default)]
        headless: Option<bool>,
        /// Extra Chromium launch arguments.
        #[serde(default)]
        args: Vec<String>,
    },

    /// Navigate the active page to a URL.
    Navigate {
        /// Target URL.
        url: String,
        /// Wait strategy after navigation.
        #[serde(default)]
        wait: WaitStrategy,
    },

    /// Capture an accessibility snapshot.
    Snapshot {
        /// Snapshot filtering options.
        #[serde(default)]
        options: SnapshotOptions,
    },

    /// Click an element by ref (`@e1`) or CSS selector.
    Click {
        /// Ref or CSS selector.
        target: String,
    },

    /// Clear and fill an input by ref or CSS selector.
    Fill {
        /// Ref or CSS selector.
        target: String,
        /// Value to fill.
        value: String,
    },

    /// Type text into the focused element or a target.
    Type {
        /// Optional ref or CSS selector (types into focused element if absent).
        target: Option<String>,
        /// Text to type.
        text: String,
    },

    /// Take a screenshot of the active page.
    Screenshot {
        /// If true, capture the full scrollable page.
        #[serde(default)]
        full_page: bool,
    },

    /// Evaluate `JavaScript` on the active page.
    Eval {
        /// JS expression to evaluate.
        expression: String,
    },

    /// Get text content of the page or a specific element.
    Text {
        /// Optional CSS selector to scope text extraction.
        selector: Option<String>,
    },

    /// Get the current page URL.
    GetUrl,

    /// Get the current page title.
    GetTitle,

    /// Go back in history.
    Back,

    /// Go forward in history.
    Forward,

    /// Reload the current page.
    Reload,

    /// Wait for a condition.
    Wait {
        /// What to wait for.
        condition: WaitCondition,
    },

    /// Hover an element by ref or CSS selector.
    Hover {
        /// Ref or CSS selector.
        target: String,
    },

    /// Focus an element by ref or CSS selector.
    Focus {
        /// Ref or CSS selector.
        target: String,
    },

    /// Check daemon health / browser status.
    Status,

    /// Close the browser and shut down the daemon.
    Close,
}

// ---------------------------------------------------------------------------
// Wait types
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
    /// Wait until no network requests for 500ms.
    NetworkIdle,
}

/// Condition the `Wait` command blocks on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WaitCondition {
    /// Wait for a CSS selector to appear in the DOM.
    Selector {
        /// CSS selector.
        selector: String,
        /// Timeout in milliseconds (default 30 000).
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for text to appear anywhere on the page.
    Text {
        /// Text to search for.
        text: String,
        /// Timeout in milliseconds.
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for the URL to match a pattern (substring match).
    Url {
        /// URL substring or pattern.
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

/// Default timeout: 30 seconds.
const fn default_timeout_ms() -> u64 {
    30_000
}

// ---------------------------------------------------------------------------
// Responses (Daemon → CLI)
// ---------------------------------------------------------------------------

/// Response sent from the daemon back to the CLI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Response {
    /// Command succeeded.
    Ok {
        /// Optional payload (depends on the command).
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
    /// Shorthand for a success response with no data.
    #[must_use]
    pub const fn ok() -> Self {
        Self::Ok { data: None }
    }

    /// Shorthand for a success response with data.
    #[must_use]
    pub const fn ok_data(data: ResponseData) -> Self {
        Self::Ok { data: Some(data) }
    }

    /// Shorthand for an error response.
    #[must_use]
    pub fn error(message: impl Into<String>) -> Self {
        Self::Error {
            message: message.into(),
        }
    }
}

/// Payload variants returned by different commands.
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
        /// Ref map as JSON object.
        refs: serde_json::Value,
    },

    /// Screenshot bytes (base64-encoded PNG).
    Screenshot {
        /// Base64-encoded image data.
        data: String,
    },

    /// `JavaScript` evaluation result.
    Eval {
        /// Serialized result value.
        value: serde_json::Value,
    },

    /// Text content.
    Text {
        /// Extracted text.
        content: String,
    },

    /// URL string.
    Url {
        /// Current page URL.
        url: String,
    },

    /// Page title.
    Title {
        /// Current page title.
        title: String,
    },

    /// Daemon status.
    Status {
        /// Whether a browser is currently running.
        browser_running: bool,
        /// Current page URL (if any).
        page_url: Option<String>,
    },
}

// ---------------------------------------------------------------------------
// Port file helpers
// ---------------------------------------------------------------------------

/// Directory where the daemon stores its port file and other runtime data.
///
/// Returns `~/.brother/` (or platform equivalent).
///
/// # Errors
///
/// Returns `None` if the home directory cannot be determined.
#[must_use]
pub fn runtime_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("brother"))
}

/// Path to the daemon port file.
///
/// The file contains the TCP port number as plain ASCII text.
#[must_use]
pub fn port_file_path() -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join("daemon.port"))
}

/// Path to the daemon PID file.
#[must_use]
pub fn pid_file_path() -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join("daemon.pid"))
}
