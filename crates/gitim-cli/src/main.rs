#![deny(warnings)]

mod output;

use std::env;
use std::process;

use clap::{Parser, Subcommand};

use gitim_client::{ensure_daemon, find_repo_root, GitimClient};
use output::OutputMode;

#[derive(Parser)]
#[command(name = "gitim", version, about = "GitIM CLI -- AI-native IM over Git")]
struct Cli {
    /// Output raw JSON instead of human-readable text
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show daemon status
    Status,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_flag(cli.json);

    match cli.command {
        Commands::Status => cmd_status(&mode).await,
    }
}

async fn cmd_status(mode: &OutputMode) {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot read current directory: {e}");
        process::exit(1);
    });

    let repo_root = match find_repo_root(&cwd) {
        Some(r) => r,
        None => {
            eprintln!("Error: not in a GitIM repository (no .gitim/ found)");
            process::exit(1);
        }
    };

    if let Err(e) = ensure_daemon(&repo_root) {
        eprintln!("Error: failed to start daemon: {e}");
        process::exit(1);
    }

    let client = GitimClient::new(&repo_root);
    match client.status().await {
        Ok(resp) => {
            let code = mode.print(&resp);
            if code != 0 {
                process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
