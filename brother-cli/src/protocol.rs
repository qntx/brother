//! JSON protocol for daemon ↔ CLI communication.
//!
//! The daemon listens on `127.0.0.1:<port>`. Each message is a single JSON
//! object terminated by `\n`. The CLI sends a [`Request`]; the daemon replies
//! with a [`Response`].

use brother::{MouseButton, ScrollDirection, SnapshotOptions};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Request  (CLI → Daemon)
// ---------------------------------------------------------------------------

/// A command sent from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    // -- Connection --------------------------------------------------------
    /// Connect to an existing browser via CDP websocket URL or debugging port.
    Connect {
        /// CDP websocket URL (e.g. `ws://127.0.0.1:9222/...`), or just a port number.
        target: String,
    },

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
    /// Take a screenshot (base64-encoded image).
    Screenshot {
        /// Capture the full scrollable page.
        #[serde(default)]
        full_page: bool,
        /// Optional CSS selector to screenshot a specific element.
        #[serde(default)]
        selector: Option<String>,
        /// Image format: `"png"` or `"jpeg"` (default `"png"`).
        #[serde(default = "default_screenshot_format")]
        format: String,
        /// JPEG quality (1-100, only for jpeg format).
        #[serde(default = "default_jpeg_quality")]
        quality: u8,
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
        /// Mouse button (default left).
        #[serde(default)]
        button: MouseButton,
        /// Number of clicks (default 1, use 2 for double-click).
        #[serde(default = "default_click_count")]
        click_count: u32,
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
        /// Delay between keystrokes in milliseconds (0 = no delay).
        #[serde(default)]
        delay_ms: u64,
    },
    /// Press a key combo (e.g. `"Enter"`, `"Control+a"`).
    Press {
        /// Key or key combo.
        key: String,
    },
    /// Select dropdown option(s) by value.
    Select {
        /// Ref or CSS selector of the `<select>` element.
        target: String,
        /// Option value(s) to select (supports multi-select).
        values: Vec<String>,
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

    // -- Frame (iframe) support ---------------------------------------------
    /// Switch execution context to a child frame by name, URL substring, or index.
    Frame {
        /// Frame selector: name, URL substring, or numeric index.
        selector: String,
    },
    /// Switch back to the main (top-level) frame.
    MainFrame,

    // -- Raw keyboard ------------------------------------------------------
    /// Hold a key down (without releasing).
    KeyDown {
        /// Key name (e.g. `"Shift"`, `"a"`).
        key: String,
    },
    /// Release a held key.
    KeyUp {
        /// Key name (e.g. `"Shift"`, `"a"`).
        key: String,
    },
    /// Insert text directly (without key events).
    InsertText {
        /// Text to insert.
        text: String,
    },

    // -- File / DOM manipulation -------------------------------------------
    /// Upload files to a file input element.
    Upload {
        /// Ref or CSS selector of the `<input type="file">`.
        target: String,
        /// File paths to upload.
        files: Vec<String>,
    },
    /// Drag one element onto another.
    Drag {
        /// Ref or CSS selector of the source element.
        source: String,
        /// Ref or CSS selector of the drop target.
        target: String,
    },
    /// Clear an input field.
    Clear {
        /// Ref or CSS selector.
        target: String,
    },
    /// Scroll an element into the visible viewport.
    ScrollIntoView {
        /// Ref or CSS selector.
        target: String,
    },
    /// Get the bounding box of an element.
    BoundingBox {
        /// Ref or CSS selector.
        target: String,
    },
    /// Set the page HTML content directly.
    SetContent {
        /// HTML content.
        html: String,
    },
    /// Export the page as PDF (headless only).
    Pdf {
        /// Output file path.
        path: String,
    },

    // -- Network interception -----------------------------------------------
    /// Intercept requests matching a URL pattern and respond with custom data or block.
    Route {
        /// URL substring to match.
        pattern: String,
        /// Action: fulfill (custom response) or abort (block).
        #[serde(default)]
        action: RouteAction,
        /// HTTP status code (for fulfill).
        #[serde(default = "default_status")]
        status: u16,
        /// Response body (for fulfill).
        #[serde(default)]
        body: String,
        /// Content-Type header (for fulfill).
        #[serde(default = "default_content_type")]
        content_type: String,
    },
    /// Remove a route by URL pattern. Pass `"*"` to remove all routes.
    Unroute {
        /// URL pattern to unroute.
        pattern: String,
    },
    /// List captured network requests (drains the buffer). Pass `"clear"` to just clear.
    Requests {
        /// Optional action: `"clear"` to clear without returning.
        #[serde(default)]
        action: Option<String>,
        /// Optional URL pattern to filter results.
        #[serde(default)]
        filter: Option<String>,
    },

    // -- Download handling ---------------------------------------------------
    /// Set the download directory path (enables downloads).
    SetDownloadPath {
        /// Absolute directory path for downloads.
        path: String,
    },
    /// List recent downloads or clear the download log.
    Downloads {
        /// Optional action: `"clear"` to clear without returning.
        #[serde(default)]
        action: Option<String>,
    },
    /// Wait for a download event and save the file.
    WaitForDownload {
        /// Optional path to save the downloaded file.
        #[serde(default)]
        path: Option<String>,
        /// Timeout in milliseconds (default 30s).
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },
    /// Wait for and capture a network response body matching a URL pattern.
    ResponseBody {
        /// URL substring to match.
        url: String,
        /// Timeout in milliseconds (default 30s).
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
    },

    // -- Clipboard -----------------------------------------------------------
    /// Read text from the clipboard.
    ClipboardRead,
    /// Write text to the clipboard.
    ClipboardWrite {
        /// Text to write.
        text: String,
    },

    // -- Environment emulation -----------------------------------------------
    /// Set the viewport size.
    Viewport {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// Emulate media features (color scheme, print/screen, reduced motion, etc.).
    EmulateMedia {
        /// Media type: `"screen"`, `"print"`, or empty to reset.
        #[serde(default)]
        media: Option<String>,
        /// Color scheme: `"light"`, `"dark"`, `"no-preference"`, or empty to reset.
        #[serde(default)]
        color_scheme: Option<String>,
        /// Reduced motion: `"reduce"`, `"no-preference"`, or empty to reset.
        #[serde(default)]
        reduced_motion: Option<String>,
        /// Forced colors: `"active"`, `"none"`, or empty to reset.
        #[serde(default)]
        forced_colors: Option<String>,
    },
    /// Toggle offline mode.
    Offline {
        /// `true` = offline, `false` = online.
        offline: bool,
    },
    /// Set extra HTTP headers for all subsequent requests.
    ExtraHeaders {
        /// Header name-value pairs as JSON string (e.g. `{"X-Custom": "val"}`).
        headers_json: String,
    },
    /// Override geolocation.
    Geolocation {
        /// Latitude.
        latitude: f64,
        /// Longitude.
        longitude: f64,
        /// Accuracy in meters (default 1.0).
        #[serde(default = "default_geo_accuracy")]
        accuracy: f64,
    },
    /// Set HTTP Basic Auth credentials.
    Credentials {
        /// Username.
        username: String,
        /// Password.
        password: String,
    },
    /// Override the browser user-agent string.
    UserAgent {
        /// User-agent string.
        user_agent: String,
    },
    /// Override the timezone.
    Timezone {
        /// IANA timezone ID (e.g. `"America/New_York"`).
        timezone_id: String,
    },
    /// Override the locale.
    Locale {
        /// Locale string (e.g. `"en-US"`).
        locale: String,
    },
    /// Grant or revoke browser permissions.
    Permissions {
        /// Permission names (e.g. `["geolocation", "notifications"]`).
        permissions: Vec<String>,
        /// Whether to grant (`true`) or deny (`false`).
        #[serde(default = "default_true")]
        grant: bool,
    },
    /// Bring the current page to front.
    BringToFront,

    // -- Script injection ----------------------------------------------------
    /// Add a script to evaluate on every new document (before page JS runs).
    AddInitScript {
        /// `JavaScript` source code.
        script: String,
    },
    /// Inject a `<script>` tag into the current page.
    AddScript {
        /// Inline JS content (mutually exclusive with `url`).
        #[serde(default)]
        content: Option<String>,
        /// External script URL (mutually exclusive with `content`).
        #[serde(default)]
        url: Option<String>,
    },
    /// Inject a `<style>` or `<link>` tag into the current page.
    AddStyle {
        /// Inline CSS content (mutually exclusive with `url`).
        #[serde(default)]
        content: Option<String>,
        /// External stylesheet URL (mutually exclusive with `content`).
        #[serde(default)]
        url: Option<String>,
    },
    /// Dispatch a DOM event on an element.
    Dispatch {
        /// Ref or CSS selector.
        target: String,
        /// Event name (e.g. `"click"`, `"input"`, `"change"`).
        event: String,
        /// Optional JSON object for `EventInit` properties.
        #[serde(default)]
        event_init: Option<String>,
    },

    // -- Misc interaction / queries ------------------------------------------
    /// Get computed styles of an element.
    Styles {
        /// Ref or CSS selector.
        target: String,
    },
    /// Select all text in an element.
    SelectAll {
        /// Ref or CSS selector.
        target: String,
    },
    /// Highlight an element with a visible overlay (for debugging).
    Highlight {
        /// Ref or CSS selector.
        target: String,
    },
    /// Move the mouse to absolute coordinates.
    MouseMove {
        /// X coordinate.
        x: f64,
        /// Y coordinate.
        y: f64,
    },
    /// Press a mouse button down at the current position.
    MouseDown {
        /// Mouse button (default left).
        #[serde(default)]
        button: MouseButton,
    },
    /// Release a mouse button at the current position.
    MouseUp {
        /// Mouse button (default left).
        #[serde(default)]
        button: MouseButton,
    },
    /// Scroll with the mouse wheel.
    Wheel {
        /// Horizontal scroll delta (pixels).
        #[serde(default)]
        delta_x: f64,
        /// Vertical scroll delta (pixels, positive = down).
        #[serde(default)]
        delta_y: f64,
        /// Optional CSS selector to hover first.
        #[serde(default)]
        selector: Option<String>,
    },
    /// Touch-tap an element.
    Tap {
        /// Ref or CSS selector.
        target: String,
    },
    /// Set an input's value directly (no events fired).
    SetValue {
        /// Ref or CSS selector.
        target: String,
        /// Value to set.
        value: String,
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

    // -- State checks -------------------------------------------------------
    /// Check if an element is visible.
    IsVisible {
        /// Ref or CSS selector.
        target: String,
    },
    /// Check if an element is enabled.
    IsEnabled {
        /// Ref or CSS selector.
        target: String,
    },
    /// Check if a checkbox/radio is checked.
    IsChecked {
        /// Ref or CSS selector.
        target: String,
    },
    /// Count elements matching a CSS selector.
    Count {
        /// CSS selector.
        selector: String,
    },

    // -- Wait --------------------------------------------------------------
    /// Wait for a condition.
    Wait {
        /// What to wait for.
        condition: WaitCondition,
    },

    // -- Dialog handling -----------------------------------------------------
    /// Get the current dialog message (if any).
    DialogMessage,
    /// Accept (OK) the current dialog, optionally with prompt text.
    DialogAccept {
        /// Text to enter for prompt dialogs.
        prompt_text: Option<String>,
    },
    /// Dismiss (Cancel) the current dialog.
    DialogDismiss,

    // -- Cookie / Storage ---------------------------------------------------
    /// Get all cookies.
    GetCookies,
    /// Set a cookie (e.g. `"name=value; path=/"`).
    SetCookie {
        /// Cookie string.
        cookie: String,
    },
    /// Clear all cookies.
    ClearCookies,
    /// Get a storage item.
    GetStorage {
        /// Key name.
        key: String,
        /// Use sessionStorage instead of localStorage.
        session: bool,
    },
    /// Set a storage item.
    SetStorage {
        /// Key name.
        key: String,
        /// Value.
        value: String,
        /// Use sessionStorage instead of localStorage.
        session: bool,
    },
    /// Clear storage.
    ClearStorage {
        /// Use sessionStorage instead of localStorage.
        session: bool,
    },

    // -- Tab management -----------------------------------------------------
    /// Open a new tab (optionally navigate to a URL).
    TabNew {
        /// URL to navigate to (defaults to `about:blank`).
        url: Option<String>,
    },
    /// List all open tabs.
    TabList,
    /// Switch to a tab by index (0-based).
    TabSelect {
        /// Tab index.
        index: usize,
    },
    /// Close a tab by index (0-based). Closes active tab if omitted.
    TabClose {
        /// Tab index (None = active).
        index: Option<usize>,
    },

    // -- Debug -------------------------------------------------------------
    /// Get captured console messages (drains the buffer).
    Console {
        /// Clear logs without returning them.
        #[serde(default)]
        clear: bool,
    },
    /// Get captured JS errors (drains the buffer).
    Errors {
        /// Clear errors without returning them.
        #[serde(default)]
        clear: bool,
    },

    // -- Lifecycle ---------------------------------------------------------
    /// Check daemon health / browser status.
    Status,
    /// Close the browser and shut down the daemon.
    Close,
}

// ---------------------------------------------------------------------------
// Route action
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

/// Default scroll distance in pixels.
const fn default_scroll_px() -> i64 {
    500
}

/// Default HTTP status for route fulfill.
const fn default_status() -> u16 {
    200
}

/// Default content type for route fulfill.
fn default_content_type() -> String {
    "text/plain".into()
}

/// Default geolocation accuracy in meters.
const fn default_geo_accuracy() -> f64 {
    1.0
}

/// Default boolean true.
const fn default_true() -> bool {
    true
}

/// Default click count.
const fn default_click_count() -> u32 {
    1
}

/// Default screenshot format.
fn default_screenshot_format() -> String {
    "png".into()
}

/// Default JPEG quality.
const fn default_jpeg_quality() -> u8 {
    80
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
