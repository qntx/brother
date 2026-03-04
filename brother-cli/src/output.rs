//! Response formatting — JSON and plain-text output.

use base64::Engine;

use crate::protocol::{Response, ResponseData};

/// Screenshot save configuration extracted from CLI args.
pub struct ScreenshotOutput {
    pub path: Option<String>,
    pub format: brother::ImageFormat,
}

/// Print a daemon response as JSON or plain text.
pub fn print_response(response: Response, json_mode: bool, screenshot: Option<&ScreenshotOutput>) {
    match response {
        Response::Ok { data } => {
            if json_mode {
                let val = response_to_json(data.as_ref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).expect("valid json")
                );
            } else {
                print_plain(data.as_ref(), screenshot);
            }
        }
        Response::Error { message } => {
            if json_mode {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "success": false, "error": message,
                    }))
                    .expect("valid json")
                );
            } else {
                eprintln!("error: {message}");
            }
        }
    }
}

/// Convert response data to a JSON value for --json output.
fn response_to_json(data: Option<&ResponseData>) -> serde_json::Value {
    data.map_or_else(
        || serde_json::json!({ "success": true }),
        |d| {
            let mut val = serde_json::to_value(d).unwrap_or(serde_json::Value::Null);
            if let serde_json::Value::Object(ref mut map) = val {
                map.insert("success".into(), serde_json::Value::Bool(true));
            }
            val
        },
    )
}

/// Print plain-text output for a response.
#[allow(clippy::cognitive_complexity)]
fn print_plain(data: Option<&ResponseData>, screenshot: Option<&ScreenshotOutput>) {
    match data {
        Some(ResponseData::Navigate { url, title }) => {
            println!("url: {url}");
            println!("title: {title}");
        }
        Some(ResponseData::Snapshot { tree, .. }) => println!("{tree}"),
        Some(ResponseData::Screenshot { data }) => {
            if let Some(ss) = screenshot {
                let path = ss.path.clone().unwrap_or_else(|| {
                    let ext = ss.format.extension();
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("screenshot-{ts}.{ext}")
                });
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(&path, &bytes) {
                            eprintln!("error writing screenshot: {e}");
                        } else {
                            println!("saved: {path} ({} bytes)", bytes.len());
                        }
                    }
                    Err(e) => eprintln!("error decoding screenshot: {e}"),
                }
            }
        }
        Some(ResponseData::Eval { value }) => println!("{value}"),
        Some(ResponseData::Text { text }) => println!("{text}"),
        Some(ResponseData::Status {
            browser_running,
            page_url,
        }) => {
            println!(
                "browser: {}",
                if *browser_running {
                    "running"
                } else {
                    "stopped"
                }
            );
            if let Some(url) = page_url {
                println!("page: {url}");
            }
        }
        Some(ResponseData::Logs { entries }) => {
            if let Some(arr) = entries.as_array() {
                if arr.is_empty() {
                    println!("(no entries)");
                } else {
                    for entry in arr {
                        if let Some(level) = entry.get("level").and_then(|v| v.as_str()) {
                            let text = entry.get("text").and_then(|v| v.as_str()).unwrap_or("");
                            println!("[{level}] {text}");
                        } else if let Some(msg) = entry.get("message").and_then(|v| v.as_str()) {
                            println!("[error] {msg}");
                        }
                    }
                }
            }
        }
        Some(ResponseData::BoundingBox {
            x,
            y,
            width,
            height,
        }) => {
            println!("x: {x}, y: {y}, width: {width}, height: {height}");
        }
        Some(ResponseData::TabList { tabs, active }) => {
            if let Some(arr) = tabs.as_array() {
                for tab in arr {
                    let idx = tab
                        .get("index")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let url = tab
                        .get("url")
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)");
                    let marker = if usize::try_from(idx).ok() == Some(*active) {
                        " *"
                    } else {
                        ""
                    };
                    println!("[{idx}] {url}{marker}");
                }
            }
        }
        Some(ResponseData::DiffSnapshot {
            diff,
            summary,
            added,
            removed,
            ..
        }) => {
            if *added == 0 && *removed == 0 {
                println!("(no changes)");
            } else {
                println!("{diff}---\n{summary}");
            }
        }
        Some(ResponseData::DiffScreenshot {
            diff_path, summary, ..
        }) => {
            println!("{summary}");
            println!("diff image: {diff_path}");
        }
        Some(ResponseData::StateList { states }) => {
            if states.is_empty() {
                println!("(no saved states)");
            } else {
                for s in states {
                    println!("  {s}");
                }
            }
        }
        Some(ResponseData::AnnotatedScreenshot { data, annotations }) => {
            if let Some(ss) = screenshot {
                let path = ss.path.clone().unwrap_or_else(|| {
                    let ext = ss.format.extension();
                    let ts = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis());
                    format!("screenshot-{ts}.{ext}")
                });
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(&path, &bytes) {
                            eprintln!("error writing screenshot: {e}");
                        } else {
                            println!("saved: {path} ({} bytes)", bytes.len());
                        }
                    }
                    Err(e) => eprintln!("error decoding screenshot: {e}"),
                }
            }
            if let Some(arr) = annotations.as_array() {
                println!("annotations: {} elements", arr.len());
                for a in arr {
                    let num = a
                        .get("number")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
                    let role = a.get("role").and_then(|v| v.as_str()).unwrap_or("");
                    let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    print!("  @e{num} [{role}]");
                    if !name.is_empty() {
                        print!(" \"{name}\"");
                    }
                    println!();
                }
            }
        }
        Some(ResponseData::AuthProfile {
            name,
            url,
            username,
            created_at,
            last_login_at,
            updated,
        }) => {
            if matches!(updated, Some(true)) {
                println!("updated: {name}");
            } else if updated.is_some() {
                println!("created: {name}");
            }
            println!("  url: {url}");
            println!("  username: {username}");
            println!("  created: {created_at}");
            if let Some(last) = last_login_at {
                println!("  last login: {last}");
            }
        }
        Some(ResponseData::AuthList { profiles }) => {
            if profiles.is_empty() {
                println!("(no auth profiles)");
            } else {
                for p in profiles {
                    let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                    let url = p.get("url").and_then(|v| v.as_str()).unwrap_or("");
                    let user = p.get("username").and_then(|v| v.as_str()).unwrap_or("");
                    println!("  {name}  {user}  {url}");
                }
            }
        }
        Some(ResponseData::ConfirmationRequired {
            confirmation_id,
            action,
            category,
            description,
        }) => {
            println!("confirmation required [{category}]");
            println!("  action: {action}");
            println!("  description: {description}");
            println!("  id: {confirmation_id}");
            println!("  approve: brother confirm {confirmation_id}");
            println!("  reject:  brother deny {confirmation_id}");
        }
        None => println!("ok"),
    }
}
