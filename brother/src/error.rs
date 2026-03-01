//! Error types for the brother library.

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

/// Convenience result type for the brother library.
pub type Result<T> = std::result::Result<T, Error>;
