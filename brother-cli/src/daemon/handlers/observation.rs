//! Observation handlers: snapshot, wait, screenshot, bounding_box, dialog, console, errors, status.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData, WaitCondition, WaitStrategy};

use crate::daemon::state::{DaemonState, get_page};

pub(in crate::daemon) async fn cmd_snapshot(
    state: &Arc<Mutex<DaemonState>>,
    options: brother::SnapshotOptions,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.snapshot_with(options).await {
        Ok(snap) => {
            let refs = serde_json::to_value(snap.refs()).unwrap_or_default();
            Response::ok_data(ResponseData::Snapshot {
                tree: snap.tree().to_owned(),
                refs,
            })
        }
        Err(e) => Response::error(format!("snapshot failed: {e}")),
    }
}

pub(in crate::daemon) async fn cmd_wait(
    state: &Arc<Mutex<DaemonState>>,
    condition: WaitCondition,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let result = match condition {
        WaitCondition::Selector {
            selector,
            timeout_ms,
        } => {
            page.wait_for_selector(&selector, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Text { text, timeout_ms } => {
            page.wait_for_text(&text, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Url {
            pattern,
            timeout_ms,
        } => {
            page.wait_for_url(&pattern, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::Function {
            expression,
            timeout_ms,
        } => {
            page.wait_for_function(&expression, Duration::from_millis(timeout_ms))
                .await
        }
        WaitCondition::LoadState {
            state: ws,
            timeout_ms,
        } => match ws {
            WaitStrategy::NetworkIdle => {
                page.wait_for_network_idle(Duration::from_millis(timeout_ms))
                    .await
            }
            _ => page.wait_for_navigation().await,
        },
        WaitCondition::Duration { ms } => {
            page.wait(Duration::from_millis(ms)).await;
            Ok(())
        }
    };
    match result {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(in crate::daemon) async fn cmd_screenshot(
    state: &Arc<Mutex<DaemonState>>,
    full_page: bool,
    selector: Option<&str>,
    format: &str,
    quality: u8,
    annotate: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    if !annotate {
        return match page
            .screenshot(full_page, selector, format, Some(quality))
            .await
        {
            Ok(bytes) => {
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Response::ok_data(ResponseData::Screenshot { data })
            }
            Err(e) => Response::error(format!("screenshot failed: {e}")),
        };
    }

    let snap = match page
        .snapshot_with(brother::SnapshotOptions::default().interactive_only(true))
        .await
    {
        Ok(s) => s,
        Err(e) => return Response::error(format!("snapshot for annotations failed: {e}")),
    };

    let refs = snap.refs();
    let mut annotations = Vec::new();
    for (ref_id, info) in refs {
        let num: u32 = ref_id
            .strip_prefix('e')
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if let Ok((x, y, w, h)) = page.bounding_box(&format!("@{ref_id}")).await {
            if w > 0.0 && h > 0.0 {
                annotations.push(serde_json::json!({
                    "ref": ref_id,
                    "number": num,
                    "role": info.role,
                    "name": info.name,
                    "box": {
                        "x": x.round(),
                        "y": y.round(),
                        "width": w.round(),
                        "height": h.round(),
                    }
                }));
            }
        }
    }

    annotations.sort_by_key(|a| a["number"].as_u64().unwrap_or(0));

    if !annotations.is_empty() {
        let overlay_data: Vec<serde_json::Value> = annotations
            .iter()
            .map(|a| {
                serde_json::json!({
                    "number": a["number"],
                    "x": a["box"]["x"],
                    "y": a["box"]["y"],
                    "width": a["box"]["width"],
                    "height": a["box"]["height"],
                })
            })
            .collect();

        let inject_js = format!(
            r#"(() => {{
var items = {items};
var id = '__brother_annotations__';
var sx = window.scrollX || 0;
var sy = window.scrollY || 0;
var c = document.createElement('div');
c.id = id;
c.style.cssText = 'position:absolute;top:0;left:0;width:0;height:0;pointer-events:none;z-index:2147483647;';
for (var i = 0; i < items.length; i++) {{
  var it = items[i];
  var dx = it.x + sx;
  var dy = it.y + sy;
  var b = document.createElement('div');
  b.style.cssText = 'position:absolute;left:'+dx+'px;top:'+dy+'px;width:'+it.width+'px;height:'+it.height+'px;border:2px solid rgba(255,0,0,0.8);box-sizing:border-box;pointer-events:none;';
  var l = document.createElement('div');
  l.textContent = String(it.number);
  var labelTop = dy < 14 ? '2px' : '-14px';
  l.style.cssText = 'position:absolute;top:'+labelTop+';left:-2px;background:rgba(255,0,0,0.9);color:#fff;font:bold 11px/14px monospace;padding:0 4px;border-radius:2px;white-space:nowrap;';
  b.appendChild(l);
  c.appendChild(b);
}}
document.documentElement.appendChild(c);
}})()"#,
            items = serde_json::to_string(&overlay_data).unwrap_or_default()
        );

        let _ = page.eval(&inject_js).await;
    }

    let screenshot_result = page
        .screenshot(full_page, selector, format, Some(quality))
        .await;

    if !annotations.is_empty() {
        let _ = page
            .eval("(() => { const el = document.getElementById('__brother_annotations__'); if (el) el.remove(); })()")
            .await;
    }

    match screenshot_result {
        Ok(bytes) => {
            let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let annot_value = serde_json::Value::Array(annotations);
            Response::ok_data(ResponseData::AnnotatedScreenshot {
                data,
                annotations: annot_value,
            })
        }
        Err(e) => Response::error(format!("screenshot failed: {e}")),
    }
}

pub(in crate::daemon) async fn cmd_bounding_box(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.bounding_box(target).await {
        Ok((x, y, w, h)) => Response::ok_data(ResponseData::BoundingBox {
            x,
            y,
            width: w,
            height: h,
        }),
        Err(e) => Response::error(e.ai_friendly(target).to_string()),
    }
}

pub(in crate::daemon) async fn cmd_dialog_message(state: &Arc<Mutex<DaemonState>>) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    page.dialog_message().await.map_or_else(
        || {
            Response::ok_data(ResponseData::Text {
                text: "(no dialog)".into(),
            })
        },
        |info| {
            let value = serde_json::to_value(&info).unwrap_or_default();
            Response::ok_data(ResponseData::Eval { value })
        },
    )
}

pub(in crate::daemon) async fn cmd_console(
    state: &Arc<Mutex<DaemonState>>,
    clear: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let logs = page.take_console_logs().await;
    if clear {
        return Response::ok_data(ResponseData::Text {
            text: format!("{} console entries cleared", logs.len()),
        });
    }
    Response::ok_data(ResponseData::Logs {
        entries: serde_json::to_value(&logs).unwrap_or_default(),
    })
}

pub(in crate::daemon) async fn cmd_errors(
    state: &Arc<Mutex<DaemonState>>,
    clear: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let errors = page.take_js_errors().await;
    if clear {
        return Response::ok_data(ResponseData::Text {
            text: format!("{} error entries cleared", errors.len()),
        });
    }
    Response::ok_data(ResponseData::Logs {
        entries: serde_json::to_value(&errors).unwrap_or_default(),
    })
}

pub(in crate::daemon) async fn cmd_status(state: &Arc<Mutex<DaemonState>>) -> Response {
    let guard = state.lock().await;
    let browser_running = guard.browser.is_some();
    let page_url = if let Some(page) = guard.pages.get(guard.active_tab) {
        page.url().await.ok()
    } else {
        None
    };
    Response::ok_data(ResponseData::Status {
        browser_running,
        page_url,
    })
}
