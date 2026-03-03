//! Brother CLI — thin client that communicates with the brother daemon.
//!
//! The daemon holds a persistent browser instance across commands, enabling
//! workflows like `open` → `snapshot` → `click` without restarting Chrome.

#![allow(clippy::print_stdout, clippy::print_stderr)]

mod client;
mod commands;
mod daemon;
mod output;
mod protocol;
mod request;

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
    if matches!(cli.command, Command::Daemon) {
        daemon::run(None).await?;
        return Ok(());
    }

    let json = cli.json;
    let mut client = DaemonClient::connect().await?;

    let req = request::build_request(&cli.command);
    let response = client.send(&req).await?;
    output::print_response(&cli.command, response, json);
    Ok(())
}
