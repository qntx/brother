//! Error types for the brother library.
//!
//! All errors are converted to AI-friendly messages via [`Error::ai_friendly`]
//! so that LLM agents receive actionable guidance instead of raw CDP traces.

/// All errors that can occur in the brother library.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Browser launch or connection failure.
    #[error("browser error: {0}")]
    Browser(String),

    /// CDP protocol error.
    #[error("cdp error: {0}")]
    Cdp(#[from] chromiumoxide::error::CdpError),

    /// Navigation failure.
    #[error("navigation error: {0}")]
    Navigation(String),

    /// Element not found by ref or selector.
    #[error("element not found: {0}")]
    ElementNotFound(String),

    /// Snapshot capture failure.
    #[error("snapshot error: {0}")]
    Snapshot(String),

    /// Page operation on a closed or invalid page.
    #[error("page closed")]
    PageClosed,

    /// Timeout waiting for an operation.
    #[error("timeout: {0}")]
    Timeout(String),

    /// Invalid argument provided by the caller.
    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// JSON serialization/deserialization error.
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// URL parse error.
    #[error("url parse error: {0}")]
    UrlParse(#[from] url::ParseError),
}

impl Error {
    /// Rewrite a raw error into an AI-agent-friendly message.
    ///
    /// `target` is the ref or selector the user supplied (e.g. `"@e3"` or
    /// `"#submit"`). It is embedded in the rewritten message to give the
    /// agent concrete context about what failed.
    ///
    /// Rules (checked in order — first match wins):
    ///
    /// | Pattern in raw message | Rewritten to |
    /// |---|---|
    /// | node not found / no node | Ref is stale → re-snapshot |
    /// | not an element | Target is not an element |
    /// | layout object / zero-size | Element not visible |
    /// | not focusable | Element not focusable |
    /// | timeout / timed out | Action timed out |
    /// | selector … not found | Selector not found → re-snapshot |
    /// | navigation | Navigation failed |
    /// | page closed | Page was closed |
    #[must_use]
    pub fn ai_friendly(self, target: &str) -> Self {
        let raw = self.to_string();
        let lower = raw.to_ascii_lowercase();

        // Stale ref — backend node no longer exists
        if lower.contains("could not find node")
            || lower.contains("no node with given id")
            || lower.contains("node not found")
        {
            return Self::ElementNotFound(format!(
                "Element \"{target}\" no longer exists in the DOM. \
                 Run 'snapshot' to get updated refs."
            ));
        }

        // Object is not an HTML element (e.g. text node)
        if lower.contains("not an element") || lower.contains("node is not an element") {
            return Self::ElementNotFound(format!(
                "Target \"{target}\" resolved to a non-element node. \
                 Use a more specific ref or CSS selector."
            ));
        }

        // Element has no layout (display:none, zero-size, off-screen)
        if lower.contains("layout object")
            || lower.contains("zero-size")
            || lower.contains("not visible")
        {
            return Self::ElementNotFound(format!(
                "Element \"{target}\" is not visible (zero size or hidden). \
                 Try scrolling into view or check if it's behind a modal."
            ));
        }

        // Element is not focusable
        if lower.contains("not focusable") || lower.contains("cannot focus") {
            return Self::InvalidArgument(format!(
                "Element \"{target}\" is not focusable. \
                 Try clicking it instead, or verify it's an interactive element."
            ));
        }

        // Pointer interception (overlay / modal blocking)
        if lower.contains("intercept") && lower.contains("pointer") {
            return Self::ElementNotFound(format!(
                "Element \"{target}\" is blocked by another element (overlay or modal). \
                 Dismiss any modals/cookie banners first."
            ));
        }

        // General timeout
        if lower.contains("timeout") || lower.contains("timed out") {
            return Self::Timeout(format!(
                "Action on \"{target}\" timed out. The element may be blocked, \
                 still loading, or not interactable. Run 'snapshot' to check the page."
            ));
        }

        // CSS selector matched nothing
        if lower.contains("not found") && lower.contains("selector") {
            return Self::ElementNotFound(format!(
                "Selector \"{target}\" matched no elements. \
                 Run 'snapshot' to see the current page elements."
            ));
        }

        // Navigation error
        if lower.contains("navigation") && lower.contains("fail") {
            return Self::Navigation(
                "Navigation failed. Check that the URL is valid and the page is reachable."
                    .to_owned(),
            );
        }

        // Page closed
        if lower.contains("page closed") || lower.contains("target closed") {
            return Self::PageClosed;
        }

        // Multiple elements matched (ambiguous selector)
        if lower.contains("strict mode violation")
            || lower.contains("resolved to") && lower.contains("elements")
        {
            return Self::InvalidArgument(format!(
                "Selector \"{target}\" matched multiple elements. \
                 Use a more specific selector or run 'snapshot' to find the exact ref."
            ));
        }

        // Detached element (removed from DOM between resolve and action)
        if lower.contains("detached") || lower.contains("orphan") {
            return Self::ElementNotFound(format!(
                "Element \"{target}\" was removed from the DOM during the action. \
                 Run 'snapshot' to get updated refs."
            ));
        }

        // Execution context destroyed (navigation happened during eval)
        if lower.contains("execution context") && lower.contains("destroy") {
            return Self::Navigation(
                "A navigation occurred while executing JavaScript. \
                 Wait for navigation to complete before evaluating scripts."
                    .to_owned(),
            );
        }

        // No pattern matched — return original
        self
    }
}

/// Convenience result type for the brother library.
pub type Result<T> = std::result::Result<T, Error>;
