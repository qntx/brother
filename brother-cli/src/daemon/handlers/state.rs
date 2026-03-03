//! State persistence handlers: save, load, list, clear, show, clean, rename.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;

use crate::protocol::{Response, ResponseData};

use crate::daemon::state::{DaemonState, get_page};

/// Directory for saved states: `~/.brother/sessions/`.
fn sessions_dir() -> Option<std::path::PathBuf> {
    crate::protocol::runtime_dir().map(|d| d.join("sessions"))
}

/// Validate a state name: only `[a-zA-Z0-9_-]` allowed.
/// Prevents path traversal attacks (e.g. `"../../etc/passwd"`).
#[allow(clippy::result_large_err)]
fn validate_state_name(name: &str) -> Result<(), Response> {
    if name == "*" {
        return Ok(()); // wildcard is allowed for clear-all
    }
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err(Response::error(format!(
            "invalid state name '{name}': only alphanumeric, hyphens, and underscores allowed"
        )));
    }
    Ok(())
}

/// Save cookies + localStorage + sessionStorage to a named JSON file.
pub(in crate::daemon) async fn cmd_state_save(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Gather cookies
    let cookies = match page.get_cookies().await {
        Ok(v) => v,
        Err(e) => return Response::error(format!("get cookies: {e}")),
    };

    // Gather localStorage + sessionStorage via JS
    let storage_js = r"(() => {
        const ls = {};
        for (let i = 0; i < localStorage.length; i++) {
            const k = localStorage.key(i);
            ls[k] = localStorage.getItem(k);
        }
        const ss = {};
        for (let i = 0; i < sessionStorage.length; i++) {
            const k = sessionStorage.key(i);
            ss[k] = sessionStorage.getItem(k);
        }
        return JSON.stringify({ localStorage: ls, sessionStorage: ss });
    })()";

    let storage_val = page.eval(storage_js).await.unwrap_or_default();
    let storage_str = storage_val.as_str().unwrap_or("{}");
    let storage: serde_json::Value =
        serde_json::from_str(storage_str).unwrap_or_else(|_| serde_json::json!({}));

    let url = page.url().await.unwrap_or_default();

    let state_data = serde_json::json!({
        "url": url,
        "cookies": cookies,
        "localStorage": storage.get("localStorage").cloned().unwrap_or_else(|| serde_json::json!({})),
        "sessionStorage": storage.get("sessionStorage").cloned().unwrap_or_else(|| serde_json::json!({})),
        "savedAt": chrono_now(),
    });

    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    if let Err(e) = tokio::fs::create_dir_all(&dir).await {
        return Response::error(format!("mkdir: {e}"));
    }

    let path = dir.join(format!("{name}.json"));
    let json = serde_json::to_string_pretty(&state_data).unwrap_or_default();
    if let Err(e) = tokio::fs::write(&path, &json).await {
        return Response::error(format!("write: {e}"));
    }

    Response::ok_data(ResponseData::Text {
        text: format!("state saved: {name} ({})", path.display()),
    })
}

/// Load a previously saved state (cookies + storage).
pub(in crate::daemon) async fn cmd_state_load(
    state: &Arc<Mutex<DaemonState>>,
    name: &str,
) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let path = dir.join(format!("{name}.json"));
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Response::error(format!("read state '{name}': {e}")),
    };
    let data: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Response::error(format!("parse state '{name}': {e}")),
    };

    let page = match get_page(state).await {
        Ok(p) => p,
        Err(r) => return r,
    };

    // Restore cookies
    if let Some(cookies) = data.get("cookies")
        && let Ok(cookie_list) =
            serde_json::from_value::<Vec<brother::CookieInput>>(cookies.clone())
        && let Err(e) = page.set_cookies(&cookie_list).await
    {
        return Response::error(format!("restore cookies: {e}"));
    }

    // Navigate to saved URL first (so storage domain matches)
    if let Some(url) = data.get("url").and_then(|v| v.as_str())
        && !url.is_empty()
        && url != "about:blank"
    {
        let _ = page.goto(url).await;
    }

    // Restore localStorage
    if let Some(ls) = data.get("localStorage").and_then(|v| v.as_object()) {
        for (k, v) in ls {
            let val = v.as_str().unwrap_or("");
            let escaped_k = k.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_v = val.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = page
                .eval(&format!(
                    "localStorage.setItem('{escaped_k}', '{escaped_v}')"
                ))
                .await;
        }
    }

    // Restore sessionStorage
    if let Some(ss) = data.get("sessionStorage").and_then(|v| v.as_object()) {
        for (k, v) in ss {
            let val = v.as_str().unwrap_or("");
            let escaped_k = k.replace('\\', "\\\\").replace('\'', "\\'");
            let escaped_v = val.replace('\\', "\\\\").replace('\'', "\\'");
            let _ = page
                .eval(&format!(
                    "sessionStorage.setItem('{escaped_k}', '{escaped_v}')"
                ))
                .await;
        }
    }

    Response::ok_data(ResponseData::Text {
        text: format!("state loaded: {name}"),
    })
}

/// List all saved state files.
pub(in crate::daemon) async fn cmd_state_list() -> Response {
    let Some(dir) = sessions_dir() else {
        return Response::ok_data(ResponseData::StateList { states: Vec::new() });
    };
    let mut names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let fname = entry.file_name().to_string_lossy().to_string();
            if let Some(name) = fname.strip_suffix(".json") {
                names.push(name.to_owned());
            }
        }
    }
    names.sort();
    Response::ok_data(ResponseData::StateList { states: names })
}

/// Delete a saved state file (or all with `name = "*"`).
pub(in crate::daemon) async fn cmd_state_clear(name: &str) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    if name == "*" {
        let mut count = 0usize;
        if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
            while let Ok(Some(entry)) = rd.next_entry().await {
                if entry.file_name().to_string_lossy().ends_with(".json") {
                    let _ = tokio::fs::remove_file(entry.path()).await;
                    count += 1;
                }
            }
        }
        Response::ok_data(ResponseData::Text {
            text: format!("{count} state(s) cleared"),
        })
    } else {
        let path = dir.join(format!("{name}.json"));
        if let Err(e) = tokio::fs::remove_file(&path).await {
            return Response::error(format!("delete state '{name}': {e}"));
        }
        Response::ok_data(ResponseData::Text {
            text: format!("state '{name}' deleted"),
        })
    }
}

/// Show the contents of a saved state file.
pub(in crate::daemon) async fn cmd_state_show(name: &str) -> Response {
    if let Err(r) = validate_state_name(name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let path = dir.join(format!("{name}.json"));
    let content = match tokio::fs::read_to_string(&path).await {
        Ok(c) => c,
        Err(e) => return Response::error(format!("read state '{name}': {e}")),
    };
    let val: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => return Response::error(format!("parse state '{name}': {e}")),
    };
    Response::ok_data(ResponseData::Eval { value: val })
}

/// Clean up state files older than `days` days.
pub(in crate::daemon) async fn cmd_state_clean(days: u32) -> Response {
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let max_age = Duration::from_secs(u64::from(days) * 86400);
    let now = std::time::SystemTime::now();
    let mut deleted = Vec::new();

    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let fname = entry.file_name().to_string_lossy().to_string();
            if !std::path::Path::new(&fname)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            if let Ok(meta) = entry.metadata().await
                && let Ok(modified) = meta.modified()
                && now.duration_since(modified).unwrap_or_default() > max_age
            {
                let _ = tokio::fs::remove_file(entry.path()).await;
                if let Some(name) = fname.strip_suffix(".json") {
                    deleted.push(name.to_owned());
                }
            }
        }
    }

    let count = deleted.len();
    Response::ok_data(ResponseData::Text {
        text: format!("{count} expired state(s) cleaned"),
    })
}

/// Rename a saved state file.
pub(in crate::daemon) async fn cmd_state_rename(old_name: &str, new_name: &str) -> Response {
    if let Err(r) = validate_state_name(old_name) {
        return r;
    }
    if let Err(r) = validate_state_name(new_name) {
        return r;
    }
    let Some(dir) = sessions_dir() else {
        return Response::error("cannot determine sessions directory");
    };
    let old_path = dir.join(format!("{old_name}.json"));
    let new_path = dir.join(format!("{new_name}.json"));
    if let Err(e) = tokio::fs::rename(&old_path, &new_path).await {
        return Response::error(format!("rename '{old_name}' → '{new_name}': {e}"));
    }
    Response::ok_data(ResponseData::Text {
        text: format!("state renamed: {old_name} → {new_name}"),
    })
}

/// Simple ISO-8601-ish timestamp without external crate.
fn chrono_now() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("{}s", d.as_secs())
}
