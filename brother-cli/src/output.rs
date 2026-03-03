//! Response formatting — JSON and plain-text output.

use base64::Engine;

use crate::commands::Command;
use crate::protocol::{Response, ResponseData};

/// Print a daemon response as JSON or plain text.
pub fn print_response(cmd: &Command, response: Response, json_mode: bool) {
    match response {
        Response::Ok { data } => {
            if json_mode {
                let val = response_to_json(data.as_ref());
                println!(
                    "{}",
                    serde_json::to_string_pretty(&val).expect("valid json")
                );
            } else {
                print_plain(cmd, data.as_ref());
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
fn print_plain(cmd: &Command, data: Option<&ResponseData>) {
    match data {
        Some(ResponseData::Navigate { url, title }) => {
            println!("url: {url}");
            println!("title: {title}");
        }
        Some(ResponseData::Snapshot { tree, .. }) => println!("{tree}"),
        Some(ResponseData::Screenshot { data }) => {
            if let Command::Screenshot { output, .. } = cmd {
                match base64::engine::general_purpose::STANDARD.decode(data) {
                    Ok(bytes) => {
                        if let Err(e) = std::fs::write(output, &bytes) {
                            eprintln!("error writing screenshot: {e}");
                        } else {
                            println!("saved: {output} ({} bytes)", bytes.len());
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
                    #[allow(clippy::cast_possible_truncation)]
                    let marker = if idx as usize == *active { " *" } else { "" };
                    println!("[{idx}] {url}{marker}");
                }
            }
        }
        None => println!("ok"),
    }
}
