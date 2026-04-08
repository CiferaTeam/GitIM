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

    /// List all channels
    Channels,

    /// Create a new channel
    CreateChannel {
        /// Channel name
        name: String,
        /// Display name
        #[arg(long)]
        display_name: Option<String>,
        /// Channel introduction
        #[arg(long)]
        introduction: Option<String>,
    },

    /// Join a channel or invite users
    JoinChannel {
        /// Channel name
        channel: String,
        /// Target handlers to invite
        #[arg(short, long, num_args = 1..)]
        targets: Vec<String>,
    },

    /// Archive a channel
    ArchiveChannel {
        /// Channel name
        name: String,
    },

    /// List archived channels
    ArchivedChannels,

    /// Direct message commands
    Dm {
        #[command(subcommand)]
        command: DmCommands,
    },

    /// Stop the daemon
    Stop,

    /// List all users
    Users,

    /// Search messages
    Search {
        /// Search query
        query: Option<String>,
        /// Filter by author handler
        #[arg(short, long)]
        author: Option<String>,
        /// Filter by channel name
        #[arg(short, long)]
        channel: Option<String>,
        /// Filter by channel type (channel or dm)
        #[arg(short = 't', long = "type")]
        channel_type: Option<String>,
        /// Maximum results to return
        #[arg(short, long, default_value = "50")]
        limit: u64,
        /// Offset for pagination
        #[arg(long, default_value = "0")]
        offset: u64,
    },

    /// Rebuild the search index
    Reindex,

    /// Onboard: clone/create repo, start daemon, register identity
    Onboard {
        /// Repository name
        repo_name: Option<String>,
        /// Organization / owner
        org: Option<String>,
        /// Git server type: git, github, gitea, gitlab
        #[arg(short = 'g', long = "git-server", default_value = "github")]
        git_server: String,
        /// Auth token for GitHub/Gitea/GitLab
        #[arg(short, long)]
        token: Option<String>,
        /// Handler (required for git local mode)
        #[arg(long)]
        handler: Option<String>,
        /// Display name (required for git local mode)
        #[arg(long)]
        display_name: Option<String>,
        /// Server URL for Gitea/GitLab
        #[arg(short, long)]
        url: Option<String>,
        /// Re-infer identity on running daemon
        #[arg(long)]
        refresh: bool,
        /// Enable HTTP debug port
        #[arg(long)]
        debug_http: bool,
        /// Admin mode
        #[arg(long)]
        admin: bool,
        /// Guest mode (read-only)
        #[arg(long)]
        guest: bool,
    },

    /// Board (kanban) commands
    Board {
        #[command(subcommand)]
        command: BoardCommands,
    },

    /// Card commands
    Card {
        #[command(subcommand)]
        command: CardCommands,
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

#[derive(Subcommand)]
enum BoardCommands {
    /// Create a new board
    Create {
        /// Board name
        name: String,
        /// Display name
        #[arg(long)]
        display_name: Option<String>,
        /// Comma-separated list of statuses
        #[arg(long)]
        statuses: Option<String>,
    },

    /// List all boards
    Ls,
}

#[derive(Subcommand)]
enum CardCommands {
    /// Create a new card
    Create {
        /// Board name
        board: String,
        /// Card title
        title: String,
        /// Assignee handler
        #[arg(long)]
        assignee: Option<String>,
        /// Initial status
        #[arg(long)]
        status: Option<String>,
    },

    /// List cards in a board
    Ls {
        /// Board name
        board: String,
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
    },

    /// Read card discussion
    Read {
        /// Board name
        board: String,
        /// Card ID
        card_id: String,
        /// Maximum number of entries
        #[arg(short, long)]
        limit: Option<u64>,
        /// Only return entries after this line number
        #[arg(short, long)]
        since: Option<u64>,
    },

    /// Send a message to a card
    Send {
        /// Board name
        board: String,
        /// Card ID
        card_id: String,
        /// Message body
        body: String,
        /// Line number to reply to
        #[arg(short, long)]
        reply_to: Option<u64>,
    },

    /// Update card status or assignee
    Update {
        /// Board name
        board: String,
        /// Card ID
        card_id: String,
        /// New status
        #[arg(long)]
        status: Option<String>,
        /// New assignee handler
        #[arg(long)]
        assignee: Option<String>,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let mode = OutputMode::from_flag(cli.json);

    // Stop handles daemon detection itself — no init_client needed
    if let Commands::Stop = &cli.command {
        commands::admin::cmd_stop(&mode).await;
        return;
    }

    // Onboard manages its own repo directory and daemon lifecycle
    if let Commands::Onboard {
        repo_name,
        org,
        git_server,
        token,
        handler,
        display_name,
        url,
        refresh,
        debug_http,
        admin,
        guest,
    } = cli.command
    {
        commands::onboard::cmd_onboard(commands::onboard::OnboardArgs {
            repo_name,
            org,
            git_server,
            token,
            handler,
            display_name,
            url,
            refresh,
            debug_http,
            admin,
            guest,
        })
        .await;
        return;
    }

    let client = init_client();

    match cli.command {
        Commands::Stop => unreachable!(),
        Commands::Onboard { .. } => unreachable!(),
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
        Commands::Channels => commands::channels::cmd_channels(&client, &mode).await,
        Commands::CreateChannel {
            name,
            display_name,
            introduction,
        } => {
            commands::channels::cmd_create_channel(
                &client,
                &mode,
                &name,
                display_name.as_deref(),
                introduction.as_deref(),
            )
            .await
        }
        Commands::JoinChannel { channel, targets } => {
            commands::channels::cmd_join_channel(&client, &mode, &channel, &targets).await
        }
        Commands::ArchiveChannel { name } => {
            commands::channels::cmd_archive_channel(&client, &mode, &name).await
        }
        Commands::ArchivedChannels => {
            commands::channels::cmd_archived_channels(&client, &mode).await
        }
        Commands::Users => commands::admin::cmd_users(&client, &mode).await,
        Commands::Search {
            query,
            author,
            channel,
            channel_type,
            limit,
            offset,
        } => {
            commands::admin::cmd_search(
                &client,
                &mode,
                query.as_deref(),
                author.as_deref(),
                channel.as_deref(),
                channel_type.as_deref(),
                limit,
                offset,
            )
            .await
        }
        Commands::Reindex => commands::admin::cmd_reindex(&client, &mode).await,
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
        Commands::Board { command } => match command {
            BoardCommands::Create {
                name,
                display_name,
                statuses,
            } => {
                let status_vec: Option<Vec<String>> = statuses
                    .map(|s| s.split(',').map(|s| s.trim().to_string()).collect());
                commands::board::cmd_create_board(
                    &client,
                    &mode,
                    &name,
                    display_name.as_deref(),
                    status_vec.as_deref(),
                )
                .await
            }
            BoardCommands::Ls => commands::board::cmd_list_boards(&client, &mode).await,
        },
        Commands::Card { command } => match command {
            CardCommands::Create {
                board,
                title,
                assignee,
                status,
            } => {
                commands::card::cmd_create_card(
                    &client,
                    &mode,
                    &board,
                    &title,
                    assignee.as_deref(),
                    status.as_deref(),
                )
                .await
            }
            CardCommands::Ls { board, status } => {
                commands::card::cmd_list_cards(&client, &mode, &board, status.as_deref()).await
            }
            CardCommands::Read {
                board,
                card_id,
                limit,
                since,
            } => {
                commands::card::cmd_read_card(&client, &mode, &board, &card_id, limit, since).await
            }
            CardCommands::Send {
                board,
                card_id,
                body,
                reply_to,
            } => {
                commands::card::cmd_send_card_message(
                    &client,
                    &mode,
                    &board,
                    &card_id,
                    &body,
                    reply_to,
                )
                .await
            }
            CardCommands::Update {
                board,
                card_id,
                status,
                assignee,
            } => {
                commands::card::cmd_update_card(
                    &client,
                    &mode,
                    &board,
                    &card_id,
                    status.as_deref(),
                    assignee.as_deref(),
                )
                .await
            }
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
