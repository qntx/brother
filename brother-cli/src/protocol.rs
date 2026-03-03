//! JSON protocol for daemon ↔ CLI communication.
//!
//! The daemon listens on `127.0.0.1:<port>`. Each message is a single JSON
//! object terminated by `\n`. The CLI sends a [`Request`]; the daemon replies
//! with a [`Response`].

use brother::{MouseButton, ScrollDirection, SnapshotOptions};
use serde::{Deserialize, Serialize};

/// A command sent from the CLI to the daemon.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    /// Configure and launch the browser with specific options.
    /// Must be sent before any other command. Ignored if browser is already running.
    Launch {
        /// Run in headed mode (show browser window). Default: false (headless).
        #[serde(default)]
        headed: bool,
        /// Proxy server URL (e.g. `http://localhost:8080`).
        #[serde(default)]
        proxy: Option<String>,
        /// Path to Chrome/Chromium executable.
        #[serde(default)]
        executable_path: Option<String>,
        /// User data directory for persistent profiles.
        #[serde(default)]
        user_data_dir: Option<String>,
        /// Additional Chrome launch arguments (comma-separated or repeated).
        #[serde(default)]
        extra_args: Vec<String>,
        /// Custom user-agent string (applied at launch time).
        #[serde(default)]
        user_agent: Option<String>,
        /// Ignore HTTPS/TLS certificate errors.
        #[serde(default)]
        ignore_https_errors: bool,
        /// Default download directory.
        #[serde(default)]
        download_path: Option<String>,
        /// Viewport width (default 1280).
        #[serde(default = "default_viewport_width")]
        viewport_width: u32,
        /// Viewport height (default 720).
        #[serde(default = "default_viewport_height")]
        viewport_height: u32,
        /// Chrome extension paths to load (headed mode only).
        #[serde(default)]
        extensions: Vec<String>,
        /// Preferred color scheme: `"light"`, `"dark"`, or `"no-preference"`.
        #[serde(default)]
        color_scheme: Option<String>,
        /// Allowed domains — navigation to other domains will be blocked at launch.
        #[serde(default)]
        allowed_domains: Vec<String>,
    },

    /// Connect to an existing browser via CDP websocket URL or debugging port.
    Connect {
        /// CDP websocket URL (e.g. `ws://127.0.0.1:9222/...`), or just a port number.
        target: String,
    },

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
        /// Delay in ms between mouse-down and mouse-up (0 = instant).
        #[serde(default)]
        delay: u64,
        /// Click with Ctrl held to open link in a new tab.
        #[serde(default)]
        new_tab: bool,
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
        /// Clear existing content before typing.
        #[serde(default)]
        clear: bool,
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

    /// Switch execution context to a child frame by name, URL substring, or index.
    Frame {
        /// Frame selector: name, URL substring, or numeric index.
        selector: String,
    },
    /// Switch back to the main (top-level) frame.
    MainFrame,

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
    /// Click an element and wait for the resulting download.
    Download {
        /// Ref or CSS selector of the element to click.
        target: String,
        /// Directory to save the downloaded file.
        #[serde(default)]
        path: Option<String>,
        /// Timeout in milliseconds (default 30s).
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u64,
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

    /// Read text from the clipboard.
    ClipboardRead,
    /// Write text to the clipboard.
    ClipboardWrite {
        /// Text to write.
        text: String,
    },

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
    /// Find elements by semantic locator and optionally act on them.
    Find {
        /// Locator type: `role`, `text`, `label`, `placeholder`, `testid`, `alttext`, `title`.
        by: String,
        /// Value to search for.
        value: String,
        /// Optional name filter (only for `role` locator).
        #[serde(default)]
        name: Option<String>,
        /// Exact text match (default false).
        #[serde(default)]
        exact: bool,
        /// Optional sub-action: `click`, `fill`, `check`, `hover`.
        /// If absent, returns matched elements without acting.
        #[serde(default)]
        subaction: Option<String>,
        /// Value for `fill` sub-action.
        #[serde(default)]
        fill_value: Option<String>,
    },

    /// Emulate a device preset (sets viewport + user-agent).
    Device {
        /// Device name (e.g. `iphone-14`, `pixel-7`, `ipad-pro`).
        name: String,
    },
    /// List all available device presets.
    DeviceList,
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

    /// Get text content (whole page or scoped by target).
    GetText {
        /// Optional ref or CSS selector.
        #[serde(default)]
        target: Option<String>,
    },
    /// Get `innerText` of an element (rendered text, excludes hidden content).
    GetInnerText {
        /// Ref or CSS selector.
        target: String,
    },
    /// Get the full page HTML (`document.documentElement.outerHTML`).
    GetContent,
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
    /// Select the nth element matching a CSS selector and optionally act on it.
    Nth {
        /// CSS selector.
        selector: String,
        /// 0-based index (-1 for last).
        index: i64,
        /// Sub-action: `click`, `fill`, `check`, `hover`, `text`.
        /// If absent, returns element info.
        #[serde(default)]
        subaction: Option<String>,
        /// Value for `fill` sub-action.
        #[serde(default)]
        fill_value: Option<String>,
    },
    /// Expose a named function to the page's `window` object.
    Expose {
        /// Function name to expose (e.g. `"myCallback"`).
        name: String,
    },

    /// Wait for a condition.
    Wait {
        /// What to wait for.
        condition: WaitCondition,
    },

    /// Get the current dialog message (if any).
    DialogMessage,
    /// Accept (OK) the current dialog, optionally with prompt text.
    DialogAccept {
        /// Text to enter for prompt dialogs.
        prompt_text: Option<String>,
    },
    /// Dismiss (Cancel) the current dialog.
    DialogDismiss,

    /// Get all cookies.
    GetCookies,
    /// Set cookies with full attribute control via CDP `Network.setCookies`.
    SetCookies {
        /// Array of structured cookie objects.
        cookies: Vec<brother::CookieInput>,
    },
    /// Set a cookie via simple string format (e.g. `"name=value; path=/"`).
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

    /// Open a new browser window (separate from tabs).
    WindowNew {
        /// Viewport width (default: inherit current).
        #[serde(default)]
        width: Option<u32>,
        /// Viewport height (default: inherit current).
        #[serde(default)]
        height: Option<u32>,
    },
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

    /// Compare the current accessibility snapshot against a baseline text.
    DiffSnapshot {
        /// Baseline snapshot text to compare against.
        baseline: String,
        /// Snapshot options for the *current* snapshot.
        #[serde(default)]
        options: SnapshotOptions,
    },
    /// Compare the current screenshot against a baseline (base64-encoded PNG).
    DiffScreenshot {
        /// Base64-encoded PNG of the baseline screenshot.
        baseline: String,
        /// Per-channel pixel threshold (0–255). Default: 10.
        #[serde(default = "default_diff_threshold")]
        threshold: u8,
        /// Take a full-page screenshot for the current state.
        #[serde(default)]
        full_page: bool,
    },

    /// Compare two URLs: navigate to each, take snapshot + optional screenshot, diff.
    DiffUrl {
        /// First URL.
        url_a: String,
        /// Second URL.
        url_b: String,
        /// Also compare screenshots pixel-by-pixel.
        #[serde(default)]
        screenshot: bool,
        /// Per-channel pixel threshold (0–255) for screenshot diff.
        #[serde(default = "default_diff_threshold")]
        threshold: u8,
        /// Snapshot options (interactive, compact, `max_depth`, selector).
        #[serde(default)]
        options: SnapshotOptions,
    },

    /// Save current browser state (cookies + storage) to a named file.
    StateSave {
        /// State name (file will be `<name>.json` in `~/.brother/sessions/`).
        name: String,
    },
    /// Load previously saved browser state.
    StateLoad {
        /// State name to load.
        name: String,
    },
    /// List all saved states.
    StateList,
    /// Delete a saved state (or all with `name = "*"`).
    StateClear {
        /// State name to delete, or `"*"` for all.
        name: String,
    },
    /// Show the contents of a saved state file.
    StateShow {
        /// State name.
        name: String,
    },
    /// Clean up state files older than N days.
    StateClean {
        /// Maximum age in days.
        days: u32,
    },
    /// Rename a saved state.
    StateRename {
        /// Current name.
        old_name: String,
        /// New name.
        new_name: String,
    },

    /// Start CDP tracing (Performance, devtools.timeline, etc.).
    TraceStart {
        /// Tracing categories (comma-separated). Default: standard set.
        #[serde(default)]
        categories: Vec<String>,
    },
    /// Stop CDP tracing and return the trace data.
    TraceStop {
        /// Optional file path to write the trace JSON.
        #[serde(default)]
        path: Option<String>,
    },
    /// Start CDP Profiler.
    ProfilerStart {
        /// Tracing categories for the profiler.
        #[serde(default)]
        categories: Vec<String>,
    },
    /// Stop CDP Profiler and return the profile.
    ProfilerStop {
        /// Optional file path to write the profile JSON.
        #[serde(default)]
        path: Option<String>,
    },

    /// Start CDP screencast (captures screen frames as base64 images).
    ScreencastStart {
        /// Image format: `jpeg` or `png`. Default: `jpeg`.
        #[serde(default = "default_screencast_format")]
        format: String,
        /// JPEG quality (1–100). Default: 80. Ignored for PNG.
        #[serde(default = "default_screencast_quality")]
        quality: u32,
        /// Max width for the captured frames.
        #[serde(default)]
        max_width: Option<u32>,
        /// Max height for the captured frames.
        #[serde(default)]
        max_height: Option<u32>,
    },
    /// Stop CDP screencast.
    ScreencastStop,

    /// Start recording HTTP traffic as HAR (HTTP Archive).
    HarStart,
    /// Stop HAR recording and save the archive.
    HarStop {
        /// File path to write the HAR JSON. If omitted, returns data inline.
        #[serde(default)]
        path: Option<String>,
    },

    /// Set allowed domains — navigation to other domains will be blocked.
    SetAllowedDomains {
        /// List of allowed domain patterns (e.g. `["example.com", "*.github.com"]`).
        domains: Vec<String>,
    },

    /// Check daemon health / browser status.
    Status,
    /// Close the browser and shut down the daemon.
    Close,
}

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

/// Default viewport width.
const fn default_viewport_width() -> u32 {
    1280
}

/// Default viewport height.
const fn default_viewport_height() -> u32 {
    720
}

/// Default pixel diff threshold.
const fn default_diff_threshold() -> u8 {
    10
}

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

/// Default screencast format.
fn default_screencast_format() -> String {
    "jpeg".into()
}

/// Default screencast quality.
const fn default_screencast_quality() -> u32 {
    80
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
const fn default_timeout_ms() -> u64 {
    30_000
}

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
}

/// Runtime directory for daemon files (`~/.brother/`).
#[must_use]
pub fn runtime_dir() -> Option<std::path::PathBuf> {
    dirs::data_local_dir().map(|d| d.join("brother"))
}

/// Path to the daemon port file for a given session.
#[must_use]
pub fn port_file_path_for(session: &str) -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join(format!("{session}.port")))
}

/// Path to the daemon PID file for a given session.
#[must_use]
pub fn pid_file_path_for(session: &str) -> Option<std::path::PathBuf> {
    runtime_dir().map(|d| d.join(format!("{session}.pid")))
}

