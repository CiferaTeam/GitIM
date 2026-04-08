#![deny(warnings)]

mod commands;
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

    /// Send a message to a channel
    Send {
        /// Channel name
        channel: String,
        /// Message body
        body: String,
        /// Author handler (defaults to current user)
        #[arg(short, long)]
        author: Option<String>,
        /// Line number to reply to
        #[arg(short, long)]
        reply_to: Option<u64>,
    },

    /// Read messages from a channel
    Read {
        /// Channel name
        channel: String,
        /// Maximum number of messages to return
        #[arg(short, long)]
        limit: Option<u64>,
        /// Only return messages after this line number
        #[arg(short, long)]
        since: Option<u64>,
    },

    /// Direct message commands
    Dm {
        #[command(subcommand)]
        command: DmCommands,
    },
}

#[derive(Subcommand)]
enum DmCommands {
    /// Send a direct message
    Send {
        /// Target handler
        handler: String,
        /// Message body
        body: String,
        /// Author handler (defaults to current user)
        #[arg(short, long)]
        author: Option<String>,
        /// Line number to reply to
        #[arg(short, long)]
        reply_to: Option<u64>,
    },

    /// Read direct messages with a user
    Read {
        /// Target handler
        handler: String,
        /// Author handler (defaults to current user)
        #[arg(short, long)]
        author: Option<String>,
        /// Maximum number of messages to return
        #[arg(short, long)]
        limit: Option<u64>,
        /// Only return messages after this line number
        #[arg(short, long)]
        since: Option<u64>,
    },

    /// List DM conversations
    List,
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_flag(cli.json);

    let client = init_client();

    match cli.command {
        Commands::Status => cmd_status(&client, &mode).await,
        Commands::Send {
            channel,
            body,
            author,
            reply_to,
        } => {
            commands::messaging::cmd_send(
                &client,
                &mode,
                &channel,
                &body,
                author.as_deref(),
                reply_to,
            )
            .await
        }
        Commands::Read {
            channel,
            limit,
            since,
        } => {
            commands::messaging::cmd_read(&client, &mode, &channel, limit, since).await
        }
        Commands::Dm { command } => match command {
            DmCommands::Send {
                handler,
                body,
                author,
                reply_to,
            } => {
                commands::dm::cmd_dm_send(
                    &client,
                    &mode,
                    &handler,
                    &body,
                    author.as_deref(),
                    reply_to,
                )
                .await
            }
            DmCommands::Read {
                handler,
                author,
                limit,
                since,
            } => {
                commands::dm::cmd_dm_read(
                    &client,
                    &mode,
                    &handler,
                    author.as_deref(),
                    limit,
                    since,
                )
                .await
            }
            DmCommands::List => commands::dm::cmd_dm_list(&mode),
        },
    }
}

fn init_client() -> GitimClient {
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

    GitimClient::new(&repo_root)
}

async fn cmd_status(client: &GitimClient, mode: &OutputMode) {
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
