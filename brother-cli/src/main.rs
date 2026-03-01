//! Brother CLI — thin client that communicates with the brother daemon.
//!
//! The daemon holds a persistent browser instance across commands, enabling
//! workflows like `open` → `snapshot` → `click` without restarting Chrome.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;

use brother::client::DaemonClient;
use brother::protocol::{Request, Response, ResponseData, WaitCondition, WaitStrategy};
use clap::{Parser, Subcommand};

/// Browser automation CLI for AI agents.
#[derive(Parser)]
#[command(name = "brother", version, about)]
struct Cli {
    /// Output as JSON (for programmatic consumption).
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

    /// Capture an accessibility snapshot with element refs.
    Snapshot {
        /// Only include interactive elements (buttons, links, inputs).
        #[arg(short, long)]
        interactive: bool,

        /// Remove empty structural nodes.
        #[arg(short, long)]
        compact: bool,
    },

    /// Click an element by ref (`@e1`) or CSS selector.
    Click {
        /// Ref or CSS selector (e.g. `@e1`, `#submit`).
        target: String,
    },

    /// Clear and fill an input by ref or CSS selector.
    Fill {
        /// Ref or CSS selector.
        target: String,
        /// Value to fill.
        value: String,
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

    /// Get page text content.
    Text {
        /// CSS selector to scope extraction (optional).
        #[arg(short, long)]
        selector: Option<String>,
    },

    /// Go back in history.
    Back,

    /// Go forward in history.
    Forward,

    /// Reload the current page.
    Reload,

    /// Wait for a condition.
    Wait {
        /// What to wait for: a CSS selector, a duration (e.g. `3000`), or a flag.
        target: Option<String>,

        /// Wait for text to appear on the page.
        #[arg(long)]
        text: Option<String>,

        /// Wait for URL to match a pattern.
        #[arg(long)]
        url: Option<String>,

        /// Wait for a load state (`load`, `domcontentloaded`, `networkidle`).
        #[arg(long)]
        load: Option<String>,

        /// Wait for a JS expression to be truthy.
        #[arg(long, name = "fn")]
        function: Option<String>,

        /// Timeout in milliseconds (default 30000).
        #[arg(short, long, default_value = "30000")]
        timeout: u64,
    },

    /// Check daemon and browser status.
    Status,

    /// Close the browser and stop the daemon.
    Close,

    /// (Hidden) Run the daemon server process.
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
    // Hidden daemon subcommand — run the server directly
    if matches!(cli.command, Command::Daemon) {
        return brother::daemon::run(None).await;
    }

    let json = cli.json;

    // Connect to daemon (auto-starts if needed)
    let mut client = DaemonClient::connect().await?;

    // Ensure browser is launched
    let launch_resp = client
        .send(&Request::Launch {
            headless: None,
            args: vec![],
        })
        .await?;

    if let Response::Error { message } = launch_resp {
        return Err(brother::Error::Browser(message));
    }

    // Build and send the command
    let request = build_request(&cli.command);
    let response = client.send(&request).await?;

    // Print the response
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
        Command::Snapshot {
            interactive,
            compact,
        } => Request::Snapshot {
            options: brother::SnapshotOptions::default()
                .interactive_only(*interactive)
                .compact(*compact),
        },
        Command::Click { target } => Request::Click {
            target: target.clone(),
        },
        Command::Fill { target, value } => Request::Fill {
            target: target.clone(),
            value: value.clone(),
        },
        Command::Screenshot { .. } => Request::Screenshot { full_page: false },
        Command::Eval { expression } => Request::Eval {
            expression: expression.clone(),
        },
        Command::Text { selector } => Request::Text {
            selector: selector.clone(),
        },
        Command::Back => Request::Back,
        Command::Forward => Request::Forward,
        Command::Reload => Request::Reload,
        Command::Wait {
            target,
            text,
            url,
            load,
            function,
            timeout,
        } => build_wait_request(target.as_deref(), text.as_deref(), url.as_deref(), load.as_deref(), function.as_deref(), *timeout),
        Command::Status | Command::Daemon => Request::Status,
        Command::Close => Request::Close,
    }
}

/// Build a Wait request from the various wait flags.
fn build_wait_request(
    target: Option<&str>,
    text: Option<&str>,
    url: Option<&str>,
    load: Option<&str>,
    function: Option<&str>,
    timeout: u64,
) -> Request {
    let condition = text.map_or_else(
        || {
            url.map_or_else(
                || {
                    load.map_or_else(
                        || {
                            function.map_or_else(
                                || {
                                    target.map_or_else(
                                        || WaitCondition::Duration { ms: timeout },
                                        |sel| {
                                            sel.parse::<u64>().map_or_else(
                                                |_| WaitCondition::Selector {
                                                    selector: sel.to_owned(),
                                                    timeout_ms: timeout,
                                                },
                                                |ms| WaitCondition::Duration { ms },
                                            )
                                        },
                                    )
                                },
                                |f| WaitCondition::Function {
                                    expression: f.to_owned(),
                                    timeout_ms: timeout,
                                },
                            )
                        },
                        |l| {
                            let state = match l {
                                "domcontentloaded" => WaitStrategy::DomContentLoaded,
                                "networkidle" => WaitStrategy::NetworkIdle,
                                _ => WaitStrategy::Load,
                            };
                            WaitCondition::LoadState {
                                state,
                                timeout_ms: timeout,
                            }
                        },
                    )
                },
                |u| WaitCondition::Url {
                    pattern: u.to_owned(),
                    timeout_ms: timeout,
                },
            )
        },
        |t| WaitCondition::Text {
            text: t.to_owned(),
            timeout_ms: timeout,
        },
    );

    Request::Wait { condition }
}

/// Print a daemon response as JSON or plain text.
fn print_response(cmd: &Command, response: Response, json_mode: bool) {
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
                        "success": false,
                        "error": message,
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
            // Add success: true at the top level
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
        Some(ResponseData::Snapshot { tree, .. }) => {
            println!("{tree}");
        }
        Some(ResponseData::Screenshot { data }) => {
            // For screenshots, we need to decode base64 and write to file
            if let Command::Screenshot { output } = cmd {
                match base64_decode(data) {
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
        Some(ResponseData::Eval { value }) => {
            println!("{value}");
        }
        Some(ResponseData::Text { content }) => {
            println!("{content}");
        }
        Some(ResponseData::Url { url }) => {
            println!("{url}");
        }
        Some(ResponseData::Title { title }) => {
            println!("{title}");
        }
        Some(ResponseData::Status {
            browser_running,
            page_url,
        }) => {
            println!("browser: {}", if *browser_running { "running" } else { "stopped" });
            if let Some(url) = page_url {
                println!("page: {url}");
            }
        }
        None => {
            println!("ok");
        }
    }
}

/// Simple base64 decoder (no extra crate).
#[allow(clippy::cast_possible_truncation)]
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    const DECODE: [u8; 128] = {
        let mut table = [255u8; 128];
        let chars = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let mut i = 0;
        while i < 64 {
            table[chars[i] as usize] = i as u8;
            i += 1;
        }
        table
    };

    let input = input.as_bytes();
    let mut output = Vec::with_capacity(input.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0u32;

    for &b in input {
        if b == b'=' || b == b'\n' || b == b'\r' {
            continue;
        }
        if b >= 128 || DECODE[b as usize] == 255 {
            return Err(format!("invalid base64 character: {}", b as char));
        }
        buf = (buf << 6) | u32::from(DECODE[b as usize]);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(output)
}
