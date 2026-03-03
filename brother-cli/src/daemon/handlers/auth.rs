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

    // Navigate to login URL
    if let Err(e) = page.goto(&profile.url).await {
        return Response::error(format!("navigation failed: {e}"));
    }

    // Fill username
    let user_sel = profile.username_selector.as_deref().unwrap_or(
        "input[type='email'], input[type='text'], input[name='username'], input[name='email']",
    );
    if let Err(e) = page.fill(user_sel, &profile.username).await {
        return Response::error(format!("failed to fill username ({user_sel}): {e}"));
    }

    // Fill password
    let pass_sel = profile
        .password_selector
        .as_deref()
        .unwrap_or("input[type='password']");
    if let Err(e) = page.fill(pass_sel, &profile.password).await {
        return Response::error(format!("failed to fill password ({pass_sel}): {e}"));
    }

    // Click submit
    let submit_sel = profile
        .submit_selector
        .as_deref()
        .unwrap_or("button[type='submit'], input[type='submit']");
    if let Err(e) = page.click(submit_sel).await {
        return Response::error(format!("failed to click submit ({submit_sel}): {e}"));
    }

    // Update last login timestamp
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
