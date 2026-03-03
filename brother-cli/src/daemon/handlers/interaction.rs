//! Interaction handlers: click, type, find, nth, expose.

use std::sync::Arc;
use std::time::Duration;

use brother::MouseButton;
use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use crate::daemon::state::{DaemonState, get_page};

pub(in crate::daemon) async fn cmd_click(
    state: &Arc<Mutex<DaemonState>>,
    target: &str,
    button: MouseButton,
    click_count: u32,
    delay: u64,
    new_tab: bool,
) -> Response {
    if !new_tab {
        let page = match get_page(state).await {
            Ok(p) => p,
            Err(r) => return r,
        };
        return match page.click_with(target, button, click_count, delay).await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly(target).to_string()),
        };
    }
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(e) = page.key_down("Control").await {
        return Response::error(e.to_string());
    }
    let click_result = page.click(target).await;
    let _ = page.key_up("Control").await;
    if let Err(e) = click_result {
        return Response::error(e.ai_friendly(target).to_string());
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut guard = state.lock().await;
    if let Some(ref browser) = guard.browser {
        if let Ok(pages) = browser.pages().await {
            for p in pages {
                let url = p.url().await.unwrap_or_default();
                if !guard
                    .pages
                    .iter()
                    .any(|ep| futures::executor::block_on(ep.url()).unwrap_or_default() == url)
                {
                    guard.pages.push(p);
                }
            }
        }
        guard.active_tab = guard.pages.len().saturating_sub(1);
    }
    Response::ok()
}

pub(in crate::daemon) async fn cmd_type(
    state: &Arc<Mutex<DaemonState>>,
    target: Option<&str>,
    text: &str,
    delay_ms: u64,
    clear: bool,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if clear && let Some(t) = target {
        return match page.fill(t, text).await {
            Ok(()) => Response::ok(),
            Err(e) => Response::error(e.ai_friendly(t).to_string()),
        };
    }
    match page.type_with_delay(target, text, delay_ms).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(in crate::daemon) async fn cmd_find(
    state: &Arc<Mutex<DaemonState>>,
    by: &str,
    value: &str,
    name: Option<&str>,
    exact: bool,
    subaction: Option<&str>,
    fill_value: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Some(sub) = subaction {
        return match page
            .locator_action(by, value, name, exact, sub, fill_value)
            .await
        {
            Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
            Err(e) => Response::error(e.to_string()),
        };
    }
    let result = match by {
        "role" => page.find_by_role(value, name).await,
        "text" => page.find_by_text(value, exact).await,
        "label" => page.find_by_label(value).await,
        "placeholder" => page.find_by_placeholder(value).await,
        "testid" => page.find_by_testid(value).await,
        "alttext" | "alt" => page.find_by_alt_text(value, exact).await,
        "title" => page.find_by_title(value, exact).await,
        _ => {
            return Response::error(format!(
                "unknown locator type '{by}'. Use: role, text, label, placeholder, testid, alttext, title"
            ));
        }
    };
    match result {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(in crate::daemon) async fn cmd_nth(
    state: &Arc<Mutex<DaemonState>>,
    selector: &str,
    index: i64,
    subaction: Option<&str>,
    fill_value: Option<&str>,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page
        .nth_action(selector, index, subaction, fill_value)
        .await
    {
        Ok(val) => Response::ok_data(ResponseData::Eval { value: val }),
        Err(e) => Response::error(e.to_string()),
    }
}

pub(in crate::daemon) async fn cmd_expose(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let escaped = name.replace('\\', "\\\\").replace('\'', "\\'");
    let js = format!(
        "window['{escaped}'] = (...args) => console.log(JSON.stringify({{ fn: '{escaped}', args }}))"
    );
    match page.add_init_script(&js).await {
        Ok(()) => {
            let _ = page.eval(&js).await;
            Response::ok_data(ResponseData::Text {
                text: format!("function '{name}' exposed on window"),
            })
        }
        Err(e) => Response::error(format!("expose: {e}")),
    }
}

pub(in crate::daemon) async fn cmd_extra_headers(
    state: &Arc<Mutex<DaemonState>>,
    headers_json: &str,
) -> Response {
    let map: serde_json::Map<String, serde_json::Value> = match serde_json::from_str(headers_json) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("invalid headers JSON: {e}")),
    };
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    match page.set_extra_headers(map).await {
        Ok(()) => Response::ok(),
        Err(e) => Response::error(e.to_string()),
    }
}
