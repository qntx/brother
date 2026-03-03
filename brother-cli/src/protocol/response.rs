//! Response types returned by the daemon to the CLI.

use serde::{Deserialize, Serialize};

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
    /// Annotated screenshot with element ref overlays.
    AnnotatedScreenshot {
        /// Base64-encoded image data (with overlays baked in).
        data: String,
        /// Array of annotation objects with ref, number, role, name, box.
        annotations: serde_json::Value,
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
    /// Console messages or JS errors (JSON array).
    Logs {
        /// Serialized log entries.
        entries: serde_json::Value,
    },
    /// Element bounding box.
    BoundingBox {
        /// X coordinate.
        x: f64,
        /// Y coordinate.
        y: f64,
        /// Width.
        width: f64,
        /// Height.
        height: f64,
    },
    /// Tab list result.
    TabList {
        /// Tab descriptions: `[{index, url, active}]`.
        tabs: serde_json::Value,
        /// Currently active tab index.
        active: usize,
    },
    /// Snapshot diff result.
    DiffSnapshot {
        /// Unified diff text.
        diff: String,
        /// Lines added.
        added: usize,
        /// Lines removed.
        removed: usize,
        /// Lines unchanged.
        unchanged: usize,
        /// Human-readable summary.
        summary: String,
    },
    /// Screenshot diff result.
    DiffScreenshot {
        /// Path to the diff image PNG (red-highlighted differences).
        diff_path: String,
        /// Total pixels compared.
        total_pixels: u64,
        /// Pixels that differ.
        diff_pixels: u64,
        /// Percentage that differ.
        diff_percentage: f64,
        /// Whether sizes mismatch.
        size_mismatch: bool,
        /// Human-readable summary.
        summary: String,
    },
    /// List of saved state names.
    StateList {
        /// State names.
        states: Vec<String>,
    },
    /// Auth profile metadata.
    AuthProfile {
        /// Profile name.
        name: String,
        /// Login URL.
        url: String,
        /// Username.
        username: String,
        /// Creation timestamp.
        created_at: String,
        /// Last login timestamp.
        #[serde(skip_serializing_if = "Option::is_none")]
        last_login_at: Option<String>,
        /// Whether the profile was updated (vs created).
        #[serde(skip_serializing_if = "Option::is_none")]
        updated: Option<bool>,
    },
    /// List of auth profiles.
    AuthList {
        /// Profile metadata entries.
        profiles: Vec<serde_json::Value>,
    },

    /// Action requires human confirmation before execution.
    ConfirmationRequired {
        /// Unique confirmation ID (use with `Confirm` / `Deny`).
        confirmation_id: String,
        /// The action that needs confirmation.
        action: String,
        /// Action category (e.g. `eval`, `download`).
        category: String,
        /// Human-readable description.
        description: String,
    },
}
