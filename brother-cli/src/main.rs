//! Brother CLI — browser automation for AI agents.

#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::process::ExitCode;
use std::time::Duration;

use clap::{Parser, Subcommand};
use futures::StreamExt;

/// Browser automation CLI for AI agents.
#[derive(Parser)]
#[command(name = "brother", version, about)]
struct Cli {
    /// Run browser in headed (visible) mode.
    #[arg(long, global = true)]
    headed: bool,

    /// Connect to existing browser via `WebSocket` URL.
    #[arg(long, global = true)]
    connect: Option<String>,

    /// Output as JSON.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Navigate to a URL and print the page info.
    Open {
        /// Target URL.
        url: String,
    },

    /// Take an accessibility snapshot of the current page.
    Snapshot {
        /// Target URL to navigate to first.
        url: String,

        /// Only include interactive elements.
        #[arg(short, long)]
        interactive: bool,

        /// Remove empty structural nodes.
        #[arg(short, long)]
        compact: bool,
    },

    /// Click an element by ref.
    Click {
        /// Target URL.
        url: String,

        /// Element ref (e.g. `e1` or `@e1`).
        #[arg(short, long)]
        r#ref: String,
    },

    /// Capture a screenshot.
    Screenshot {
        /// Target URL.
        url: String,

        /// Output file path.
        #[arg(short, long, default_value = "screenshot.png")]
        output: String,
    },

    /// Evaluate JS on a page.
    Eval {
        /// Target URL.
        url: String,

        /// JS expression to evaluate.
        expression: String,
    },

    /// Get page text content.
    Text {
        /// Target URL.
        url: String,

        /// CSS selector (optional).
        #[arg(short, long)]
        selector: Option<String>,
    },
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

/// JSON or plain-text output helper.
fn output(json_mode: bool, json_fn: impl FnOnce() -> serde_json::Value, plain_fn: impl FnOnce()) {
    if json_mode {
        let val = json_fn();
        // Unwrap is safe: serde_json::Value always serializes
        println!(
            "{}",
            serde_json::to_string_pretty(&val).expect("valid json")
        );
    } else {
        plain_fn();
    }
}

async fn run(cli: Cli) -> brother::Result<()> {
    let config = brother::BrowserConfig::default().headless(!cli.headed);
    let json = cli.json;

    let (browser, mut handler) = if let Some(ref ws_url) = cli.connect {
        brother::Browser::connect(ws_url).await?
    } else {
        brother::Browser::launch(config).await?
    };

    tokio::spawn(async move { while handler.next().await.is_some() {} });

    match cli.command {
        Command::Open { url } => cmd_open(&browser, &url, json).await,
        Command::Snapshot {
            url,
            interactive,
            compact,
        } => cmd_snapshot(&browser, &url, interactive, compact, json).await,
        Command::Click { url, r#ref } => cmd_click(&browser, &url, &r#ref, json).await,
        Command::Screenshot { url, output } => cmd_screenshot(&browser, &url, &output, json).await,
        Command::Eval { url, expression } => cmd_eval(&browser, &url, &expression, json).await,
        Command::Text { url, selector } => {
            cmd_text(&browser, &url, selector.as_deref(), json).await
        }
    }
}

async fn cmd_open(browser: &brother::Browser, url: &str, json: bool) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(500)).await;

    let title = page.title().await?;
    let final_url = page.url().await?;

    output(
        json,
        || serde_json::json!({ "url": final_url, "title": title }),
        || {
            println!("url: {final_url}");
            println!("title: {title}");
        },
    );
    Ok(())
}

async fn cmd_snapshot(
    browser: &brother::Browser,
    url: &str,
    interactive: bool,
    compact: bool,
    json: bool,
) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(500)).await;

    let opts = brother::SnapshotOptions::default()
        .interactive_only(interactive)
        .compact(compact);
    let snapshot = page.snapshot_with(opts).await?;

    output(
        json,
        || {
            serde_json::json!({
                "tree": snapshot.tree(),
                "ref_count": snapshot.ref_count(),
                "refs": snapshot.refs(),
            })
        },
        || println!("{snapshot}"),
    );
    Ok(())
}

async fn cmd_click(
    browser: &brother::Browser,
    url: &str,
    ref_id: &str,
    json: bool,
) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(500)).await;

    let _snapshot = page.snapshot().await?;
    page.click_ref(ref_id).await?;
    page.wait(Duration::from_millis(500)).await;

    let final_url = page.url().await?;
    let title = page.title().await?;

    output(
        json,
        || serde_json::json!({ "clicked": ref_id, "url": final_url, "title": title }),
        || {
            println!("clicked: {ref_id}");
            println!("url: {final_url}");
            println!("title: {title}");
        },
    );
    Ok(())
}

async fn cmd_screenshot(
    browser: &brother::Browser,
    url: &str,
    output_path: &str,
    json: bool,
) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(1000)).await;

    let data = page.screenshot_png().await?;
    tokio::fs::write(output_path, &data)
        .await
        .map_err(|e| brother::Error::Browser(format!("failed to write screenshot: {e}")))?;

    let len = data.len();
    output(
        json,
        || serde_json::json!({ "file": output_path, "bytes": len }),
        || println!("saved: {output_path} ({len} bytes)"),
    );
    Ok(())
}

async fn cmd_eval(
    browser: &brother::Browser,
    url: &str,
    expression: &str,
    json: bool,
) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(500)).await;

    let result = page.eval(expression).await?;

    output(json, || result.clone(), || println!("{result}"));
    Ok(())
}

async fn cmd_text(
    browser: &brother::Browser,
    url: &str,
    selector: Option<&str>,
    json: bool,
) -> brother::Result<()> {
    let page = browser.new_page(url).await?;
    page.wait(Duration::from_millis(500)).await;

    let text = if let Some(sel) = selector {
        page.eval_as::<String>(&format!("document.querySelector('{sel}')?.innerText || ''"))
            .await?
    } else {
        page.eval_as::<String>("document.body.innerText").await?
    };

    output(
        json,
        || serde_json::json!({ "text": text }),
        || println!("{text}"),
    );
    Ok(())
}
