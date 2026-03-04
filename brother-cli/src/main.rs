//! Brother CLI — thin client that communicates with the brother daemon.
//!
//! The daemon holds a persistent browser instance across commands, enabling
//! workflows like `open` → `snapshot` → `click` without restarting Chrome.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod auth_vault;
mod client;
mod commands;
mod config;
mod daemon;
mod domain_filter;
mod output;
mod policy;
mod protocol;
mod request;
#[allow(dead_code)]
mod stream_server;

use std::process::ExitCode;

use crate::client::DaemonClient;
use crate::commands::Command;

use clap::Parser;

/// Browser automation CLI for AI agents.
#[derive(Parser)]
#[command(name = "brother", version, about)]
struct Cli {
    /// Output as JSON.
    #[arg(long, global = true)]
    json: bool,

    /// Run browser in headed mode (show window).
    #[arg(long, global = true)]
    headed: bool,

    /// Proxy server URL (e.g. `http://localhost:8080`).
    #[arg(long, global = true)]
    proxy: Option<String>,

    /// Path to Chrome/Chromium executable.
    #[arg(long, global = true)]
    executable_path: Option<String>,

    /// User data directory for persistent browser profiles.
    #[arg(long, global = true)]
    user_data_dir: Option<String>,

    /// Additional Chrome launch arguments.
    #[arg(long = "arg", global = true)]
    extra_args: Vec<String>,

    /// Custom user-agent string (applied at launch time).
    #[arg(long = "user-agent", global = true)]
    launch_user_agent: Option<String>,

    /// Ignore HTTPS/TLS certificate errors.
    #[arg(long, global = true)]
    ignore_https_errors: bool,

    /// Default download directory.
    #[arg(long, global = true)]
    download_path: Option<String>,

    /// Auto-discover and connect to a running Chrome instance.
    #[arg(long, global = true)]
    auto_connect: bool,

    /// Named session for daemon isolation (default: "default").
    #[arg(long, global = true, default_value = "default")]
    session: String,

    #[command(subcommand)]
    command: Command,
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

async fn run(cli: Cli) -> anyhow::Result<()> {
    // Load config from file + env vars.
    let cfg = config::load();
    let session = if cli.session == "default" {
        cfg.session.clone().unwrap_or_else(|| "default".to_owned())
    } else {
        cli.session.clone()
    };

    if matches!(cli.command, Command::Daemon) {
        daemon::run_session(&session, None, cfg.policy_file.as_deref()).await?;
        return Ok(());
    }

    let json = cli.json || cfg.json.unwrap_or(false);
    let launch = build_launch(&cli, &cfg);

    let mut client = DaemonClient::connect_session(&session).await?;

    // Send Launch request if any launch options are configured.
    if let Some(req) = launch {
        let _ = client.send(&req).await?;
    }

    // Auto-connect to running Chrome if --auto-connect flag is set.
    let auto_connect = cli.auto_connect || cfg.auto_connect.unwrap_or(false);
    if auto_connect && !matches!(cli.command, Command::AutoConnect | Command::Connect { .. }) {
        let _ = client.send(&protocol::Request::AutoConnect).await?;
    }

    let screenshot_out = match &cli.command {
        Command::Screenshot { output, format, .. } => Some(output::ScreenshotOutput {
            path: output.clone(),
            format: *format,
        }),
        _ => None,
    };

    let req = request::build_request(cli.command);
    let response = client.send(&req).await?;
    output::print_response(response, json, screenshot_out.as_ref());
    Ok(())
}

/// Build a `Launch` request by merging config-file/env values with CLI flags.
/// Returns `None` if everything is default (no configuration needed).
fn build_launch(cli: &Cli, cfg: &config::Config) -> Option<protocol::Request> {
    let headed = cli.headed || cfg.headed.unwrap_or(false);
    let proxy = cli.proxy.clone().or_else(|| cfg.proxy.clone());
    let executable_path = cli
        .executable_path
        .clone()
        .or_else(|| cfg.executable_path.clone());
    let user_data_dir = cli
        .user_data_dir
        .clone()
        .or_else(|| cfg.user_data_dir.clone());
    let user_agent = cli
        .launch_user_agent
        .clone()
        .or_else(|| cfg.user_agent.clone());
    let ignore_https_errors = cli.ignore_https_errors || cfg.ignore_https_errors.unwrap_or(false);
    let download_path = cli
        .download_path
        .clone()
        .or_else(|| cfg.download_path.clone());

    // Merge extra args: CLI flags + config file args (space-split).
    let mut extra_args = cli.extra_args.clone();
    if let Some(ref args_str) = cfg.args {
        for arg in args_str.split_whitespace() {
            if !extra_args.contains(&arg.to_owned()) {
                extra_args.push(arg.to_owned());
            }
        }
    }

    // Skip Launch if everything is default.
    if !headed
        && proxy.is_none()
        && executable_path.is_none()
        && user_data_dir.is_none()
        && extra_args.is_empty()
        && user_agent.is_none()
        && !ignore_https_errors
        && download_path.is_none()
    {
        return None;
    }

    Some(protocol::Request::Launch {
        headed,
        proxy,
        executable_path,
        user_data_dir,
        extra_args,
        user_agent,
        ignore_https_errors,
        download_path,
        viewport_width: 1280,
        viewport_height: 720,
        extensions: Vec::new(),
        color_scheme: None,
        allowed_domains: Vec::new(),
        allow_file_access: false,
        storage_state: None,
    })
}
