//! CLI command definitions (clap subcommands).

use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Connect to an existing browser (requires Chrome launched with --remote-debugging-port).
    Connect {
        /// CDP target: port number (e.g. `9222`), ws:// URL, or http:// URL.
        #[arg(default_value = "9222")]
        target: String,
    },
    /// Check daemon and browser status.
    Status,
    /// Close the browser and stop the daemon.
    Close,
    /// (Hidden) Run the daemon server.
    #[command(hide = true)]
    Daemon,

    /// Navigate to a URL.
    Open {
        /// Target URL.
        url: String,
        /// Extra HTTP header in `key:value` format (repeatable).
        #[arg(short = 'H', long = "header")]
        headers: Vec<String>,
    },
    /// Go back in history.
    Back,
    /// Go forward in history.
    Forward,
    /// Reload the current page.
    Reload,
    /// Switch to a child frame (iframe) by name, URL, or index.
    Frame {
        /// Frame name, URL substring, or numeric index.
        selector: String,
    },
    /// Switch back to the main (top-level) frame.
    MainFrame,

    /// Capture an accessibility snapshot.
    Snapshot {
        /// Only interactive elements.
        #[arg(short, long)]
        interactive: bool,
        /// Remove empty structural nodes.
        #[arg(short, long)]
        compact: bool,
        /// Maximum tree depth (0 = unlimited).
        #[arg(short, long, default_value_t = 0)]
        depth: usize,
        /// CSS selector to scope the snapshot subtree.
        #[arg(short, long)]
        selector: Option<String>,
        /// Also detect cursor-interactive elements (cursor:pointer, onclick).
        #[arg(short = 'C', long)]
        cursor: bool,
    },
    /// Capture a screenshot.
    Screenshot {
        /// Output file path (auto-generated if omitted).
        #[arg(short, long)]
        output: Option<String>,
        /// Capture the full scrollable page.
        #[arg(long)]
        full_page: bool,
        /// CSS selector to screenshot a specific element.
        #[arg(short, long)]
        selector: Option<String>,
        /// Image format: `png` or `jpeg`.
        #[arg(short, long, default_value = "png")]
        format: String,
        /// JPEG quality (1-100).
        #[arg(short, long, default_value = "80")]
        quality: u8,
        /// Annotate interactive elements with ref numbers on the screenshot.
        #[arg(short, long)]
        annotate: bool,
    },
    /// Evaluate a `JavaScript` expression.
    Eval {
        /// JS expression.
        expression: String,
    },
    /// Get text content of the page or an element.
    #[command(name = "get")]
    Get {
        /// What to get: `text`, `innertext`, `content`, `url`, `title`, `html`, `value`, `attribute`.
        what: String,
        /// Optional target (ref or CSS selector).
        target: Option<String>,
        /// Attribute name (for `get attribute`).
        #[arg(short, long)]
        attr: Option<String>,
    },
    /// Query element state: visible, enabled, checked, or count elements.
    #[command(name = "query", visible_alias = "is")]
    Query {
        /// What to check: `visible`, `enabled`, `checked`, `count`.
        what: String,
        /// Ref or CSS selector.
        target: String,
    },
    /// Get bounding box (x, y, width, height) of an element.
    BoundingBox {
        /// Ref or CSS selector.
        target: String,
    },
    /// Get computed styles of an element.
    Styles {
        /// Ref or CSS selector.
        target: String,
    },
    /// Highlight an element with a red border (for debugging).
    Highlight {
        /// Ref or CSS selector.
        target: String,
    },

    /// Click an element by ref (`@e1`) or CSS selector.
    Click {
        /// Ref or CSS selector.
        target: String,
        /// Mouse button: `left`, `right`, `middle`.
        #[arg(short, long, default_value = "left")]
        button: String,
        /// Number of clicks (use 2 for double-click).
        #[arg(short = 'n', long, default_value = "1")]
        click_count: u32,
        /// Delay in ms between mouse-down and mouse-up (0 = instant).
        #[arg(long, default_value = "0")]
        delay: u64,
        /// Ctrl+click to open the link in a new tab.
        #[arg(long)]
        new_tab: bool,
    },
    /// Double-click an element.
    Dblclick {
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
    /// Type text (append by default, use `--clear` to clear first).
    Type {
        /// Text to type.
        text: String,
        /// Optional ref or CSS selector to focus first.
        #[arg(short, long)]
        target: Option<String>,
        /// Delay between keystrokes in ms (0 = no delay).
        #[arg(short, long, default_value = "0")]
        delay: u64,
        /// Clear existing content before typing.
        #[arg(long)]
        clear: bool,
    },
    /// Press a key combo (e.g. `Enter`, `Control+a`).
    Press {
        /// Key or key combo.
        key: String,
    },
    /// Select dropdown option(s) by value.
    Select {
        /// Ref or CSS selector of the `<select>`.
        target: String,
        /// Option value(s) to select (supports multi-select).
        #[arg(required = true)]
        values: Vec<String>,
    },
    /// Check a checkbox.
    Check {
        /// Ref or CSS selector.
        target: String,
    },
    /// Uncheck a checkbox.
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
        /// Direction: `up`, `down`, `left`, `right`.
        direction: String,
        /// Pixels to scroll (default 500).
        #[arg(short, long, default_value = "500")]
        pixels: i64,
        /// Optional target to scroll.
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Scroll an element into view.
    ScrollIntoView {
        /// Ref or CSS selector.
        target: String,
    },
    /// Clear an input field.
    Clear {
        /// Ref or CSS selector.
        target: String,
    },
    /// Set an input value directly (no events).
    SetValue {
        /// Ref or CSS selector.
        target: String,
        /// Value to set.
        value: String,
    },
    /// Drag one element onto another.
    Drag {
        /// Source ref or CSS selector.
        source: String,
        /// Drop target ref or CSS selector.
        target: String,
    },
    /// Upload files to a file input.
    Upload {
        /// Ref or CSS selector of the `<input type="file">`.
        target: String,
        /// File paths to upload.
        #[arg(required = true)]
        files: Vec<String>,
    },
    /// Touch-tap an element.
    Tap {
        /// Ref or CSS selector.
        target: String,
    },
    /// Select all text in an element.
    SelectAll {
        /// Ref or CSS selector.
        target: String,
    },

    /// Hold a key down (without releasing).
    KeyDown {
        /// Key name (e.g. `Shift`, `a`).
        key: String,
    },
    /// Release a held key.
    KeyUp {
        /// Key name (e.g. `Shift`, `a`).
        key: String,
    },
    /// Insert text directly (no key events).
    InsertText {
        /// Text to insert.
        text: String,
    },
    /// Low-level mouse control: move, down, up.
    #[command(subcommand)]
    Mouse(MouseSub),
    /// Scroll with the mouse wheel.
    Wheel {
        /// Vertical scroll delta (pixels, positive = down).
        #[arg(default_value = "0")]
        delta_y: f64,
        /// Horizontal scroll delta.
        #[arg(short = 'x', long, default_value = "0")]
        delta_x: f64,
        /// Optional CSS selector to hover first.
        #[arg(short, long)]
        selector: Option<String>,
    },

    /// Find elements by semantic locator and optionally act on them.
    Find {
        /// Locator type: `role`, `text`, `label`, `placeholder`, `testid`, `alttext`, `title`.
        by: String,
        /// Value to search for.
        value: String,
        /// Name filter (only for `role` locator).
        #[arg(short, long)]
        name: Option<String>,
        /// Exact match (for `text`, `alttext`, `title` locators).
        #[arg(long)]
        exact: bool,
        /// Sub-action: `click`, `fill`, `check`, `hover`. If omitted, just list matches.
        #[arg(short, long)]
        subaction: Option<String>,
        /// Value for `fill` sub-action.
        #[arg(long)]
        fill_value: Option<String>,
    },
    /// Select the nth element and optionally act on it (0-indexed, -1 for last).
    Nth {
        /// CSS selector.
        selector: String,
        /// 0-based index (-1 for last).
        index: i64,
        /// Sub-action: `click`, `fill`, `check`, `hover`, `text`.
        #[arg(short, long)]
        subaction: Option<String>,
        /// Value for `fill` sub-action.
        #[arg(long)]
        fill_value: Option<String>,
    },
    /// Expose a named function to the page's `window` object.
    Expose {
        /// Function name (e.g. `myCallback`).
        name: String,
    },

    /// Wait for a condition.
    Wait {
        /// CSS selector, duration (ms), or omit for flag-based wait.
        target: Option<String>,
        /// Wait for text to appear.
        #[arg(long)]
        text: Option<String>,
        /// Wait for URL to match.
        #[arg(long)]
        url: Option<String>,
        /// Wait for load state (`load`|`domcontentloaded`|`networkidle`).
        #[arg(long)]
        load: Option<String>,
        /// Wait for a JS expression to be truthy.
        #[arg(long, name = "fn")]
        function: Option<String>,
        /// Timeout in ms (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },

    /// Emulate a device preset (viewport + user-agent).
    Device {
        /// Device name (e.g. `iphone-14`, `pixel-7`, `ipad-pro`, `desktop-hd`).
        name: String,
    },
    /// List all available device presets.
    DeviceList,
    /// Set viewport size.
    Viewport {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
    /// Emulate media features (color scheme, print, reduced motion, forced colors).
    EmulateMedia {
        /// Media type: `screen`, `print`.
        #[arg(short, long)]
        media: Option<String>,
        /// Color scheme: `light`, `dark`, `no-preference`.
        #[arg(short, long)]
        color_scheme: Option<String>,
        /// Reduced motion: `reduce`, `no-preference`.
        #[arg(short, long)]
        reduced_motion: Option<String>,
        /// Forced colors: `active`, `none`.
        #[arg(short, long)]
        forced_colors: Option<String>,
    },
    /// Toggle offline mode.
    Offline {
        /// `true` or `false`.
        offline: bool,
    },
    /// Set extra HTTP headers (JSON string).
    ExtraHeaders {
        /// JSON object, e.g. `{"X-Custom": "value"}`.
        headers_json: String,
    },
    /// Override geolocation.
    Geolocation {
        /// Latitude.
        latitude: f64,
        /// Longitude.
        longitude: f64,
        /// Accuracy in meters.
        #[arg(short, long, default_value = "1.0")]
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
        /// IANA timezone ID (e.g. `America/New_York`).
        timezone_id: String,
    },
    /// Override the locale.
    Locale {
        /// Locale string (e.g. `en-US`).
        locale: String,
    },
    /// Grant or revoke browser permissions.
    Permissions {
        /// Permission names (e.g. `geolocation`, `notifications`).
        #[arg(required = true)]
        permissions: Vec<String>,
        /// Deny instead of grant.
        #[arg(long)]
        deny: bool,
    },
    /// Bring the current page to front.
    BringToFront,

    /// Add a script to run on every new document (before page JS).
    AddInitScript {
        /// `JavaScript` source code.
        script: String,
    },
    /// Inject a `<script>` tag into the current page.
    AddScript {
        /// Inline JS content.
        #[arg(short, long)]
        content: Option<String>,
        /// External script URL.
        #[arg(short, long)]
        url: Option<String>,
    },
    /// Inject a `<style>` or `<link>` tag into the current page.
    AddStyle {
        /// Inline CSS content.
        #[arg(short, long)]
        content: Option<String>,
        /// External stylesheet URL.
        #[arg(short, long)]
        url: Option<String>,
    },
    /// Dispatch a DOM event on an element.
    Dispatch {
        /// Ref or CSS selector.
        target: String,
        /// Event name (e.g. `click`, `input`, `change`).
        event: String,
        /// Optional JSON `EventInit` (e.g. `{"bubbles":true}`).
        #[arg(short, long)]
        init: Option<String>,
    },
    /// Set the page HTML content directly.
    SetContent {
        /// HTML content.
        html: String,
    },
    /// Export the page as PDF.
    Pdf {
        /// Output file path.
        #[arg(default_value = "page.pdf")]
        path: String,
        /// Paper format: `letter`, `legal`, `tabloid`, `a0`–`a6`.
        #[arg(short, long)]
        format: Option<String>,
    },

    /// Dialog handling: message, accept, dismiss.
    #[command(subcommand)]
    Dialog(DialogSub),
    /// Cookie management: get, set, clear.
    #[command(subcommand)]
    Cookie(CookieSub),
    /// Storage management: get, set, clear.
    #[command(subcommand)]
    Storage(StorageSub),
    /// Clipboard: read, write.
    #[command(subcommand)]
    Clipboard(ClipboardSub),

    /// Open a new browser window.
    WindowNew {
        /// Viewport width.
        #[arg(long)]
        width: Option<u32>,
        /// Viewport height.
        #[arg(long)]
        height: Option<u32>,
    },
    /// Tab management: new, list, select, close.
    #[command(subcommand)]
    Tab(TabSub),

    /// Intercept network requests matching a URL pattern.
    Route {
        /// URL substring to match.
        pattern: String,
        /// Action: `fulfill` or `abort`.
        #[arg(short, long, default_value = "abort")]
        action: String,
        /// HTTP status code (for fulfill).
        #[arg(short, long, default_value = "200")]
        status: u16,
        /// Response body (for fulfill).
        #[arg(short, long, default_value = "")]
        body: String,
        /// Content-Type header (for fulfill).
        #[arg(short, long, default_value = "text/plain")]
        content_type: String,
    },
    /// Remove a network route. Use `*` to remove all.
    Unroute {
        /// URL pattern to remove, or `*` for all.
        pattern: String,
    },
    /// List captured network requests.
    Requests {
        /// Optional: `clear` to clear the buffer.
        action: Option<String>,
        /// URL pattern to filter results.
        #[arg(short, long)]
        filter: Option<String>,
    },
    /// Set download directory path.
    SetDownloadPath {
        /// Absolute directory path.
        path: String,
    },
    /// List files in the download directory.
    Downloads {
        /// Optional: `clear` to clear the log.
        action: Option<String>,
    },
    /// Click an element and download the resulting file.
    #[command(name = "download")]
    DownloadClick {
        /// Ref or CSS selector of the element to click.
        target: String,
        /// Path to save the downloaded file.
        #[arg(short, long)]
        path: Option<String>,
        /// Timeout in ms (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },
    /// Wait for a download to complete.
    WaitForDownload {
        /// Optional path to save the file.
        #[arg(short, long)]
        path: Option<String>,
        /// Timeout in ms (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },
    /// Wait for and capture a network response body matching a URL pattern.
    ResponseBody {
        /// URL substring to match.
        url: String,
        /// Timeout in ms (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },
    /// Set allowed domains for navigation (security filter).
    AllowedDomains {
        /// Domain patterns (e.g. `example.com *.github.com`).
        domains: Vec<String>,
    },

    /// Get captured console messages (drains buffer).
    Console {
        /// Clear logs without returning them.
        #[arg(long)]
        clear: bool,
    },
    /// Get captured JS errors (drains buffer).
    Errors {
        /// Clear errors without returning them.
        #[arg(long)]
        clear: bool,
    },
    /// CDP tracing: start, stop.
    #[command(subcommand)]
    Trace(TraceSub),
    /// CDP Profiler: start, stop.
    #[command(subcommand)]
    Profiler(ProfilerSub),
    /// CDP screencast: start, stop.
    #[command(subcommand)]
    Screencast(ScreencastSub),
    /// HAR (HTTP Archive) recording: start, stop.
    #[command(subcommand)]
    Har(HarSub),

    /// Compare current snapshot against a baseline file or text.
    #[command(name = "diff")]
    DiffSnapshot {
        /// Subcommand: `snapshot` or `screenshot`.
        #[command(subcommand)]
        sub: DiffSub,
    },
    /// Approve a pending action (from policy confirmation).
    #[command(name = "confirm")]
    Confirm {
        /// Confirmation ID.
        id: String,
    },
    /// Reject a pending action (from policy confirmation).
    #[command(name = "deny")]
    DenyAction {
        /// Confirmation ID.
        id: String,
    },

    /// Save/load browser state (cookies + storage).
    #[command(subcommand)]
    State(StateSub),

    /// Manage encrypted auth profiles (save/login/list/delete/show).
    #[command(subcommand)]
    Auth(AuthSub),

    /// Inject raw CDP input events (pair browsing / stream server).
    #[command(subcommand)]
    Input(InputSub),
}

/// Diff subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum DiffSub {
    /// Compare accessibility snapshots.
    Snapshot {
        /// Path to baseline snapshot file.
        /// If omitted, reads from stdin.
        #[arg(short, long)]
        baseline: Option<String>,

        /// Show only interactive elements in the current snapshot.
        #[arg(short, long)]
        interactive: bool,

        /// Compact output (skip structural containers).
        #[arg(short, long)]
        compact: bool,
    },
    /// Compare screenshots pixel-by-pixel.
    Screenshot {
        /// Path to baseline screenshot PNG file.
        baseline: String,

        /// Per-channel pixel threshold (0–255).
        #[arg(short, long, default_value = "10")]
        threshold: u8,

        /// Full-page screenshot.
        #[arg(short, long)]
        full_page: bool,
    },
    /// Compare two URLs (snapshot + optional screenshot).
    Url {
        /// First URL.
        url_a: String,
        /// Second URL.
        url_b: String,
        /// Also compare screenshots.
        #[arg(short, long)]
        screenshot: bool,
        /// Per-channel pixel threshold (0–255).
        #[arg(short, long, default_value = "10")]
        threshold: u8,
        /// Only include interactive elements in diff snapshot.
        #[arg(short, long)]
        interactive: bool,
        /// Compact output (skip structural containers).
        #[arg(short, long)]
        compact: bool,
        /// Maximum depth for the snapshot tree.
        #[arg(long)]
        depth: Option<usize>,
        /// CSS selector to scope the snapshot to a subtree.
        #[arg(long)]
        selector: Option<String>,
    },
}

/// State management subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum StateSub {
    /// Save cookies + localStorage + sessionStorage to a named file.
    Save {
        /// State name.
        name: String,
    },
    /// Load a previously saved state.
    Load {
        /// State name.
        name: String,
    },
    /// List all saved states.
    List,
    /// Delete a saved state (use `*` to delete all).
    Clear {
        /// State name or `*` for all.
        name: String,
    },
    /// Show the contents of a saved state.
    Show {
        /// State name.
        name: String,
    },
    /// Clean up state files older than N days.
    Clean {
        /// Maximum age in days.
        days: u32,
    },
    /// Rename a saved state.
    Rename {
        /// Current name.
        old_name: String,
        /// New name.
        new_name: String,
    },
}

/// Dialog subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum DialogSub {
    /// Get the current dialog message.
    Message,
    /// Accept the dialog (optionally with prompt text).
    Accept {
        /// Prompt text (for prompt dialogs).
        text: Option<String>,
    },
    /// Dismiss the dialog.
    Dismiss,
}

/// Cookie subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum CookieSub {
    /// Get all cookies.
    Get,
    /// Set a cookie.
    Set {
        /// Cookie string (e.g. `"name=value; path=/"`).
        value: String,
    },
    /// Clear all cookies.
    Clear,
}

/// Storage subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum StorageSub {
    /// Get a storage item by key.
    Get {
        /// Key name.
        key: String,
        /// Use sessionStorage instead of localStorage.
        #[arg(short, long)]
        session: bool,
    },
    /// Set a storage item.
    Set {
        /// Key name.
        key: String,
        /// Value.
        value: String,
        /// Use sessionStorage instead of localStorage.
        #[arg(short, long)]
        session: bool,
    },
    /// Clear all storage.
    Clear {
        /// Use sessionStorage instead of localStorage.
        #[arg(short, long)]
        session: bool,
    },
}

/// Tab subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum TabSub {
    /// Open a new tab.
    New {
        /// URL to open (defaults to about:blank).
        url: Option<String>,
    },
    /// List all open tabs.
    List,
    /// Switch to a tab by index.
    Select {
        /// Tab index (0-based).
        index: usize,
    },
    /// Close a tab by index.
    Close {
        /// Tab index (0-based, defaults to active tab).
        index: Option<usize>,
    },
}

/// Trace subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum TraceSub {
    /// Start CDP tracing.
    Start {
        /// Tracing categories (comma-separated).
        #[arg(short, long)]
        categories: Option<String>,
    },
    /// Stop CDP tracing and save output.
    Stop {
        /// File path to write trace output.
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Profiler subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ProfilerSub {
    /// Start CDP Profiler.
    Start {
        /// Profiler categories (comma-separated).
        #[arg(short, long)]
        categories: Option<String>,
    },
    /// Stop CDP Profiler and save output.
    Stop {
        /// File path to write profile output.
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Screencast subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ScreencastSub {
    /// Start screen frame capture.
    Start {
        /// Image format: `jpeg` or `png`.
        #[arg(long, default_value = "jpeg")]
        format: String,
        /// JPEG quality (1–100).
        #[arg(long, default_value = "80")]
        quality: u32,
        /// Max width for captured frames.
        #[arg(long)]
        max_width: Option<u32>,
        /// Max height for captured frames.
        #[arg(long)]
        max_height: Option<u32>,
    },
    /// Stop screen frame capture.
    Stop,
}

/// HAR subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum HarSub {
    /// Start HAR recording.
    Start,
    /// Stop HAR recording and save output.
    Stop {
        /// File path to write the HAR JSON.
        #[arg(short, long)]
        output: Option<String>,
    },
}

/// Clipboard subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum ClipboardSub {
    /// Read text from the clipboard.
    Read,
    /// Write text to the clipboard.
    Write {
        /// Text to write.
        text: String,
    },
}

/// Mouse subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum MouseSub {
    /// Move the mouse to absolute coordinates.
    Move {
        /// X coordinate.
        x: f64,
        /// Y coordinate.
        y: f64,
    },
    /// Press a mouse button down.
    Down {
        /// Button: `left`, `right`, `middle`.
        #[arg(short, long, default_value = "left")]
        button: String,
    },
    /// Release a mouse button.
    Up {
        /// Button: `left`, `right`, `middle`.
        #[arg(short, long, default_value = "left")]
        button: String,
    },
}

/// Raw CDP input injection subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum InputSub {
    /// Inject a raw mouse event.
    Mouse {
        /// Event type: `mousePressed`, `mouseReleased`, `mouseMoved`, `mouseWheel`.
        event_type: String,
        /// X coordinate.
        x: f64,
        /// Y coordinate.
        y: f64,
        /// Button: `left`, `right`, `middle`, `none`.
        #[arg(short, long)]
        button: Option<String>,
        /// Click count.
        #[arg(long)]
        click_count: Option<i64>,
        /// Wheel delta X.
        #[arg(long)]
        delta_x: Option<f64>,
        /// Wheel delta Y.
        #[arg(long)]
        delta_y: Option<f64>,
        /// Modifier flags (1=Alt, 2=Ctrl, 4=Meta, 8=Shift).
        #[arg(short, long)]
        modifiers: Option<i64>,
    },
    /// Inject a raw keyboard event.
    Keyboard {
        /// Event type: `keyDown`, `keyUp`, `char`.
        event_type: String,
        /// Key value (e.g. `Enter`, `a`).
        #[arg(short, long)]
        key: Option<String>,
        /// Key code (e.g. `KeyA`, `Enter`).
        #[arg(short, long)]
        code: Option<String>,
        /// Text generated by the keystroke.
        #[arg(short, long)]
        text: Option<String>,
        /// Modifier flags.
        #[arg(short, long)]
        modifiers: Option<i64>,
    },
    /// Inject a raw touch event.
    Touch {
        /// Event type: `touchStart`, `touchEnd`, `touchMove`, `touchCancel`.
        event_type: String,
        /// Touch points as JSON array `[[x,y],...]`.
        #[arg(short, long)]
        points: Option<String>,
        /// Modifier flags.
        #[arg(short, long)]
        modifiers: Option<i64>,
    },
}

/// Auth profile subcommands.
#[derive(Debug, Clone, clap::Subcommand)]
pub enum AuthSub {
    /// Save or update an encrypted auth profile.
    Save {
        /// Profile name.
        name: String,
        /// Login page URL.
        #[arg(short, long)]
        url: String,
        /// Username.
        #[arg(short = 'U', long)]
        username: String,
        /// Password.
        #[arg(short, long)]
        password: String,
        /// CSS selector for the username input.
        #[arg(long)]
        username_selector: Option<String>,
        /// CSS selector for the password input.
        #[arg(long)]
        password_selector: Option<String>,
        /// CSS selector for the submit button.
        #[arg(long)]
        submit_selector: Option<String>,
    },
    /// Log in using a saved auth profile.
    Login {
        /// Profile name.
        name: String,
    },
    /// List all saved auth profiles.
    List,
    /// Delete an auth profile.
    Delete {
        /// Profile name.
        name: String,
    },
    /// Show auth profile details (no password).
    Show {
        /// Profile name.
        name: String,
    },
}
