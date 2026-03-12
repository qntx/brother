//! Auth vault handlers: save, login, list, delete, show.

use std::sync::Arc;

use tokio::sync::Mutex;

use crate::auth_vault;
use crate::daemon::state::{DaemonState, get_page};
use crate::protocol::{Response, ResponseData};

pub(in crate::daemon) fn cmd_auth_save(
    name: &str,
    url: &str,
    username: &str,
    password: &str,
    username_selector: Option<&str>,
    password_selector: Option<&str>,
    submit_selector: Option<&str>,
) -> Response {
    match auth_vault::save_profile(
        name,
        url,
        username,
        password,
        username_selector,
        password_selector,
        submit_selector,
    ) {
        Ok((meta, updated)) => Response::ok_data(ResponseData::AuthProfile {
            name: meta.name,
            url: meta.url,
            username: meta.username,
            created_at: meta.created_at,
            last_login_at: meta.last_login_at,
            updated: Some(updated),
        }),
        Err(e) => Response::error(e),
    }
}

const AUTO_USER_SELECTORS: &[&str] = &[
    "input[autocomplete='username']",
    "input[type='email']",
    "input[name='username']",
    "input[name='email']",
    "input[type='text']",
];

const AUTO_SUBMIT_SELECTORS: &[&str] = &["button[type='submit']", "input[type='submit']"];

/// Try each selector in order, returning the first one whose element is visible.
async fn detect_selector(page: &brother::Page, candidates: &[&str]) -> Option<String> {
    for &sel in candidates {
        if page.is_visible(sel).await.unwrap_or(false) {
            return Some(sel.to_owned());
        }
    }
    None
}

pub(in crate::daemon) async fn cmd_auth_login(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    let profile = match auth_vault::get_profile(name) {
        Ok(Some(p)) => p,
        Ok(None) => return Response::error(format!("auth profile '{name}' not found")),
        Err(e) => return Response::error(e),
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };

    if let Err(e) = page.goto(&profile.url).await {
        return Response::error(format!("navigation failed: {e}"));
    }

    let user_sel = if let Some(ref s) = profile.username_selector {
        s.clone()
    } else {
        match detect_selector(&page, AUTO_USER_SELECTORS).await {
            Some(s) => s,
            None => {
                return Response::error(format!(
                    "auth login failed for '{name}': could not find username field. \
                 Specify --username-selector with auth save."
                ));
            }
        }
    };
    if let Err(e) = page.fill(&user_sel, &profile.username).await {
        return Response::error(format!("failed to fill username ({user_sel}): {e}"));
    }

    let pass_sel = profile
        .password_selector
        .as_deref()
        .unwrap_or("input[type='password']");
    if let Err(e) = page.fill(pass_sel, &profile.password).await {
        return Response::error(format!("failed to fill password ({pass_sel}): {e}"));
    }

    let submit_sel = if let Some(ref s) = profile.submit_selector {
        s.clone()
    } else {
        match detect_selector(&page, AUTO_SUBMIT_SELECTORS).await {
            Some(s) => s,
            None => {
                return Response::error(format!(
                    "auth login failed for '{name}': could not find submit button. \
                 Specify --submit-selector with auth save."
                ));
            }
        }
    };
    if let Err(e) = page.click(&submit_sel).await {
        return Response::error(format!("failed to click submit ({submit_sel}): {e}"));
    }

    let _ = page.wait_for_navigation().await;
    let _ = auth_vault::update_last_login(name);

    Response::ok_data(ResponseData::Text {
        text: format!("logged in as {} on {}", profile.username, profile.url),
    })
}

pub(in crate::daemon) fn cmd_auth_list() -> Response {
    match auth_vault::list_profiles() {
        Ok(profiles) => {
            let entries: Vec<serde_json::Value> = profiles
                .into_iter()
                .map(|p| {
                    serde_json::json!({
                        "name": p.name,
                        "url": p.url,
                        "username": p.username,
                        "created_at": p.created_at,
                        "last_login_at": p.last_login_at,
                    })
                })
                .collect();
            Response::ok_data(ResponseData::AuthList { profiles: entries })
        }
        Err(e) => Response::error(e),
    }
}

pub(in crate::daemon) fn cmd_auth_delete(name: &str) -> Response {
    match auth_vault::delete_profile(name) {
        Ok(true) => Response::ok(),
        Ok(false) => Response::error(format!("auth profile '{name}' not found")),
        Err(e) => Response::error(e),
    }
}

pub(in crate::daemon) fn cmd_auth_show(name: &str) -> Response {
    match auth_vault::get_profile_meta(name) {
        Ok(Some(meta)) => Response::ok_data(ResponseData::AuthProfile {
            name: meta.name,
            url: meta.url,
            username: meta.username,
            created_at: meta.created_at,
            last_login_at: meta.last_login_at,
            updated: None,
        }),
        Ok(None) => Response::error(format!("auth profile '{name}' not found")),
        Err(e) => Response::error(e),
    }
}
