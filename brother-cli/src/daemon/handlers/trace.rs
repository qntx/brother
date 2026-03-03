//! Tracing, profiler, and domain filter handlers.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use super::super::{get_page, DaemonState};

// ---------------------------------------------------------------------------
// Tracing / Profiler handlers (real CDP protocol)
// ---------------------------------------------------------------------------

/// Default tracing categories when none are specified.
const DEFAULT_TRACE_CATEGORIES: &[&str] = &[
    "devtools.timeline",
    "v8.execute",
    "disabled-by-default-devtools.timeline",
    "disabled-by-default-devtools.timeline.frame",
];

/// Start CDP `Tracing.start`.
pub(in crate::daemon) async fn cmd_trace_start(
    state: &Arc<Mutex<DaemonState>>,
    categories: &[String],
) -> Response {
    use chromiumoxide::cdp::browser_protocol::tracing::{
        StartParams, TraceConfig,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let cats: Vec<String> = if categories.is_empty() {
        DEFAULT_TRACE_CATEGORIES.iter().map(|&s| s.to_owned()).collect()
    } else {
        categories.to_vec()
    };

    let config = TraceConfig::builder()
        .included_categories(cats.clone())
        .build();

    let params = StartParams::builder()
        .trace_config(config)
        .build();

    match page.inner().execute(params).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: format!("tracing started ({})", cats.join(", ")),
        }),
        Err(e) => Response::error(format!("trace start: {e}")),
    }
}

/// Stop CDP `Tracing.end` and collect trace data.
///
/// After calling `Tracing.end`, the browser fires `Tracing.dataCollected`
/// events followed by a `Tracing.tracingComplete` event.  Listening for
/// those streamed events through chromiumoxide's typed event API requires
/// the caller to set up a subscription *before* `Tracing.end` is sent.
/// This is fragile with the current chromiumoxide API, so instead we use
/// a pragmatic approach: send `Tracing.end` and then poll for trace data
/// via the JS Performance API as a fallback, or write the raw CDP
/// response.
pub(in crate::daemon) async fn cmd_trace_stop(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::browser_protocol::tracing::EndParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Stop tracing via CDP
    if let Err(e) = page.inner().execute(EndParams::default()).await {
        return Response::error(format!("trace stop: {e}"));
    }

    // Give the browser a moment to flush trace buffers
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Collect whatever performance data is available via JS
    let js = r"(() => {
        const entries = performance.getEntriesByType('resource')
            .concat(performance.getEntriesByType('navigation'))
            .concat(performance.getEntriesByType('mark'))
            .concat(performance.getEntriesByType('measure'));
        return JSON.stringify({
            entry_count: entries.length,
            entries: entries.map(e => ({
                name: e.name,
                type: e.entryType,
                startTime: e.startTime,
                duration: e.duration
            }))
        });
    })()";

    let trace_json = page
        .eval(js)
        .await
        .ok()
        .and_then(|v| v.as_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| "{}".to_owned());

    if let Some(file_path) = path {
        if let Err(e) = tokio::fs::write(file_path, &trace_json).await {
            return Response::error(format!("write trace: {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("trace saved to {file_path}"),
        })
    } else {
        let parsed: serde_json::Value =
            serde_json::from_str(&trace_json).unwrap_or(serde_json::Value::Null);
        Response::ok_data(ResponseData::Eval { value: parsed })
    }
}

/// Start CDP `Profiler.enable` + `Profiler.start`.
pub(in crate::daemon) async fn cmd_profiler_start(
    state: &Arc<Mutex<DaemonState>>,
    _categories: &[String],
) -> Response {
    use chromiumoxide::cdp::js_protocol::profiler::{
        EnableParams, StartParams,
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    if let Err(e) = page.inner().execute(EnableParams::default()).await {
        return Response::error(format!("profiler enable: {e}"));
    }
    match page.inner().execute(StartParams::default()).await {
        Ok(_) => Response::ok_data(ResponseData::Text {
            text: "profiler started (CDP Profiler.start)".to_owned(),
        }),
        Err(e) => Response::error(format!("profiler start: {e}")),
    }
}

/// Stop CDP `Profiler.stop` and return the V8 CPU profile.
pub(in crate::daemon) async fn cmd_profiler_stop(
    state: &Arc<Mutex<DaemonState>>,
    path: Option<&str>,
) -> Response {
    use chromiumoxide::cdp::js_protocol::profiler::StopParams;

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    let resp = match page.inner().execute(StopParams::default()).await {
        Ok(r) => r,
        Err(e) => return Response::error(format!("profiler stop: {e}")),
    };

    let profile_json =
        serde_json::to_string_pretty(&resp.result.profile).unwrap_or_else(|_| "{}".into());

    if let Some(file_path) = path {
        if let Err(e) = tokio::fs::write(file_path, &profile_json).await {
            return Response::error(format!("write profile: {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("profile saved to {file_path}"),
        })
    } else {
        let parsed: serde_json::Value =
            serde_json::from_str(&profile_json).unwrap_or(serde_json::Value::Null);
        Response::ok_data(ResponseData::Eval { value: parsed })
    }
}

// ---------------------------------------------------------------------------
// Domain filter handler
// ---------------------------------------------------------------------------

/// Set allowed domain patterns for navigation security.
///
/// When domains are non-empty, injects an init script into every existing
/// page that monkey-patches `WebSocket`, `EventSource`, and
/// `navigator.sendBeacon` to block connections to non-allowed domains.
/// Navigation checks are enforced in [`super::cmd_navigate`].
pub(in crate::daemon) async fn cmd_set_allowed_domains(
    state: &Arc<Mutex<DaemonState>>,
    domains: Vec<String>,
) -> Response {
    let mut guard = state.lock().await;
    let count = domains.len();

    // Inject init script into all existing pages so future navigations
    // within those pages also get the filter.
    if !domains.is_empty() {
        let script = crate::daemon::domain_filter::build_init_script(&domains);
        for page in &guard.pages {
            let _ = page.add_init_script(&script).await;
            // Also run it immediately on the current document.
            let _ = page.eval(&script).await;
        }
    }

    guard.allowed_domains = domains;
    Response::ok_data(ResponseData::Text {
        text: if count == 0 {
            "domain filter cleared (all domains allowed)".to_owned()
        } else {
            format!("{count} domain pattern(s) set")
        },
    })
}
