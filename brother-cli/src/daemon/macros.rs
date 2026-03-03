//! Convenience macros for dispatching page methods to protocol responses.

/// Execute a page method returning `Result<()>` → `Response::ok()` or error.
///
/// With a target: `page_ok!(state, "target", method(args))` — applies AI-friendly rewrite.
/// Without target: `page_ok!(state, method(args))` — raw error.
macro_rules! page_ok {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(()) => $crate::protocol::Response::ok(),
            Err(e) => $crate::protocol::Response::error(e.ai_friendly($target).to_string()),
        }
    }};
    ($state:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(()) => $crate::protocol::Response::ok(),
            Err(e) => $crate::protocol::Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<serde_json::Value>` → `ResponseData::Eval`.
macro_rules! page_eval {
    ($state:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(val) => $crate::protocol::Response::ok_data($crate::protocol::ResponseData::Eval { value: val }),
            Err(e) => $crate::protocol::Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<String>` → `ResponseData::Text`.
///
/// With a target: `page_text!(state, "target", method(args))` — applies AI-friendly rewrite.
/// Without target: `page_text!(state, method(args))` — raw error.
macro_rules! page_text {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(text) => $crate::protocol::Response::ok_data($crate::protocol::ResponseData::Text { text }),
            Err(e) => $crate::protocol::Response::error(e.ai_friendly($target).to_string()),
        }
    }};
    ($state:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(text) => $crate::protocol::Response::ok_data($crate::protocol::ResponseData::Text { text }),
            Err(e) => $crate::protocol::Response::error(e.to_string()),
        }
    }};
}

/// Execute a page method returning `Result<impl ToString>` with a target → `ResponseData::Text`.
macro_rules! page_display {
    ($state:expr, $target:expr, $($call:tt)*) => {{
        let page = match $crate::daemon::state::get_page($state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        match page.$($call)*.await {
            Ok(val) => $crate::protocol::Response::ok_data($crate::protocol::ResponseData::Text { text: val.to_string() }),
            Err(e) => $crate::protocol::Response::error(e.ai_friendly($target).to_string()),
        }
    }};
}

pub(crate) use page_display;
pub(crate) use page_eval;
pub(crate) use page_ok;
pub(crate) use page_text;
