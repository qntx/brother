//! Brother CLI — thin client that communicates with the brother daemon.
//!
//! The daemon holds a persistent browser instance across commands, enabling
//! workflows like `open` → `snapshot` → `click` without restarting Chrome.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use base64::Engine;
use brother::client::DaemonClient;
use brother::protocol::{
    Request, Response, ResponseData, ScrollDirection, WaitCondition, WaitStrategy,
};
use clap::{Parser, Subcommand};

/// Browser automation CLI for AI agents.
#[derive(Parser)]
#[command(name = "brother", version, about)]
struct Cli {
    /// Output as JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Navigate to a URL.
    Open {
        /// Target URL.
        url: String,
    },
    /// Capture an accessibility snapshot.
    Snapshot {
        /// Only interactive elements.
        #[arg(short, long)]
        interactive: bool,
        /// Remove empty structural nodes.
        #[arg(short, long)]
        compact: bool,
    },
    /// Click an element by ref (`@e1`) or CSS selector.
    Click {
        /// Ref or CSS selector.
        target: String,
    },
    /// Double-click an element.
    Dblclick {
        /// Ref or CSS selector.
        target: String,
    },
    /// Clear and fill an input.
    Fill {
        /// Ref or CSS selector.
        target: String,
        /// Value to fill.
        value: String,
    },
    /// Type text (append, no clear).
    Type {
        /// Text to type.
        text: String,
        /// Optional ref or CSS selector to focus first.
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Press a key combo (e.g. `Enter`, `Control+a`).
    Press {
        /// Key or key combo.
        key: String,
    },
    /// Select a dropdown option by value.
    Select {
        /// Ref or CSS selector of the `<select>`.
        target: String,
        /// Option value to select.
        value: String,
    },
    /// Check a checkbox.
    Check {
        /// Ref or CSS selector.
        target: String,
    },
    /// Uncheck a checkbox.
    Uncheck {
        /// Ref or CSS selector.
        target: String,
    },
    /// Hover an element.
    Hover {
        /// Ref or CSS selector.
        target: String,
    },
    /// Focus an element.
    Focus {
        /// Ref or CSS selector.
        target: String,
    },
    /// Scroll the page or an element.
    Scroll {
        /// Direction: `up`, `down`, `left`, `right`.
        direction: String,
        /// Pixels to scroll (default 500).
        #[arg(short, long, default_value = "500")]
        pixels: i64,
        /// Optional target to scroll.
        #[arg(short, long)]
        target: Option<String>,
    },
    /// Capture a screenshot.
    Screenshot {
        /// Output file path.
        #[arg(short, long, default_value = "screenshot.png")]
        output: String,
    },
    /// Evaluate a `JavaScript` expression.
    Eval {
        /// JS expression.
        expression: String,
    },
    /// Get text content of the page or an element.
    #[command(name = "get")]
    Get {
        /// What to get: `text`, `url`, `title`, `html`, `value`, `attribute`.
        what: String,
        /// Optional target (ref or CSS selector).
        target: Option<String>,
        /// Attribute name (for `get attribute`).
        #[arg(short, long)]
        attr: Option<String>,
    },
    /// Go back in history.
    Back,
    /// Go forward in history.
    Forward,
    /// Reload the current page.
    Reload,
    /// Wait for a condition.
    Wait {
        /// CSS selector, duration (ms), or omit for flag-based wait.
        target: Option<String>,
        /// Wait for text to appear.
        #[arg(long)]
        text: Option<String>,
        /// Wait for URL to match.
        #[arg(long)]
        url: Option<String>,
        /// Wait for load state (`load`|`domcontentloaded`|`networkidle`).
        #[arg(long)]
        load: Option<String>,
        /// Wait for a JS expression to be truthy.
        #[arg(long, name = "fn")]
        function: Option<String>,
        /// Timeout in ms (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },
    /// Check daemon and browser status.
    Status,
    /// Close the browser and stop the daemon.
    Close,
    /// (Hidden) Run the daemon server.
    #[command(hide = true)]
    Daemon,
}

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("brother=info".parse().expect("valid directive")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> brother::Result<()> {
    if matches!(cli.command, Command::Daemon) {
        return brother::daemon::run(None).await;
    }

    let json = cli.json;
    let mut client = DaemonClient::connect().await?;

    let request = build_request(&cli.command);
    let response = client.send(&request).await?;
    print_response(&cli.command, response, json);
    Ok(())
}

/// Map CLI subcommand to daemon protocol request.
fn build_request(cmd: &Command) -> Request {
    match cmd {
        Command::Open { url } => Request::Navigate {
            url: url.clone(),
            wait: WaitStrategy::Load,
        },
        Command::Snapshot { interactive, compact } => Request::Snapshot {
            options: brother::SnapshotOptions::default()
                .interactive_only(*interactive)
                .compact(*compact),
        },
        Command::Click { target } => Request::Click { target: target.clone() },
        Command::Dblclick { target } => Request::DblClick { target: target.clone() },
        Command::Fill { target, value } => Request::Fill {
            target: target.clone(),
            value: value.clone(),
        },
        Command::Type { text, target } => Request::Type {
            target: target.clone(),
            text: text.clone(),
        },
        Command::Press { key } => Request::Press { key: key.clone() },
        Command::Select { target, value } => Request::Select {
            target: target.clone(),
            value: value.clone(),
        },
        Command::Check { target } => Request::Check { target: target.clone() },
        Command::Uncheck { target } => Request::Uncheck { target: target.clone() },
        Command::Hover { target } => Request::Hover { target: target.clone() },
        Command::Focus { target } => Request::Focus { target: target.clone() },
        Command::Scroll { direction, pixels, target } => Request::Scroll {
            direction: parse_direction(direction),
            pixels: *pixels,
            target: target.clone(),
        },
        Command::Screenshot { .. } => Request::Screenshot { full_page: false },
        Command::Eval { expression } => Request::Eval { expression: expression.clone() },
        Command::Get { what, target, attr } => build_get_request(what, target.as_deref(), attr.as_deref()),
        Command::Back => Request::Back,
        Command::Forward => Request::Forward,
        Command::Reload => Request::Reload,
        Command::Wait { target, text, url, load, function, timeout } => {
            build_wait_request(target.as_deref(), text.as_deref(), url.as_deref(), load.as_deref(), function.as_deref(), *timeout)
        }
        Command::Status | Command::Daemon => Request::Status,
        Command::Close => Request::Close,
    }
}

fn parse_direction(s: &str) -> ScrollDirection {
    match s.to_ascii_lowercase().as_str() {
        "up" => ScrollDirection::Up,
        "left" => ScrollDirection::Left,
        "right" => ScrollDirection::Right,
        _ => ScrollDirection::Down,
    }
}

fn build_get_request(what: &str, target: Option<&str>, attr: Option<&str>) -> Request {
    match what {
        "url" => Request::GetUrl,
        "title" => Request::GetTitle,
        "html" => Request::GetHtml {
            target: target.unwrap_or("body").to_owned(),
        },
        "value" => Request::GetValue {
            target: target.unwrap_or("input").to_owned(),
        },
        "attribute" | "attr" => Request::GetAttribute {
            target: target.unwrap_or("body").to_owned(),
            attribute: attr.unwrap_or("class").to_owned(),
        },
        // Default: get text
        _ => Request::GetText {
            target: target.map(str::to_owned),
        },
    }
}

#[allow(clippy::option_if_let_else)] // Explicit priority chain is clearer than nested map_or_else.
fn build_wait_request(
    target: Option<&str>,
    text: Option<&str>,
    url: Option<&str>,
    load: Option<&str>,
    function: Option<&str>,
    timeout: u64,
) -> Request {
    // Priority: explicit flags first, then positional target
    let condition = if let Some(t) = text {
        WaitCondition::Text { text: t.to_owned(), timeout_ms: timeout }
    } else if let Some(u) = url {
        WaitCondition::Url { pattern: u.to_owned(), timeout_ms: timeout }
    } else if let Some(f) = function {
        WaitCondition::Function { expression: f.to_owned(), timeout_ms: timeout }
    } else if let Some(l) = load {
        let state = match l {
            "domcontentloaded" => WaitStrategy::DomContentLoaded,
            "networkidle" => WaitStrategy::NetworkIdle,
            _ => WaitStrategy::Load,
        };
        WaitCondition::LoadState { state, timeout_ms: timeout }
    } else if let Some(sel) = target {
        // Numeric → duration; otherwise → CSS selector
        sel.parse::<u64>().map_or_else(
            |_| WaitCondition::Selector { selector: sel.to_owned(), timeout_ms: timeout },
            |ms| WaitCondition::Duration { ms },
        )
    } else {
        WaitCondition::Duration { ms: timeout }
    };
    Request::Wait { condition }
}

/// Print a daemon response as JSON or plain text.
fn print_response(cmd: &Command, response: Response, json_mode: bool) {
    match response {
        Response::Ok { data } => {
            if json_mode {
                let val = response_to_json(data.as_ref());
                println!("{}", serde_json::to_string_pretty(&val).expect("valid json"));
            } else {
                print_plain(cmd, data.as_ref());
            }
        }
        Response::Error { message } => {
            if json_mode {
                println!("{}", serde_json::to_string_pretty(&serde_json::json!({
                    "success": false, "error": message,
                })).expect("valid json"));
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
            if let Command::Screenshot { output } = cmd {
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
        Some(ResponseData::Status { browser_running, page_url }) => {
            println!("browser: {}", if *browser_running { "running" } else { "stopped" });
            if let Some(url) = page_url {
                println!("page: {url}");
            }
        }
        None => println!("ok"),
    }
}
