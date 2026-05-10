#![deny(warnings)]

mod commands;
mod output;

use std::env;
use std::io::Read;
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
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        body: Option<String>,
        /// Read message body from stdin
        #[arg(long)]
        stdin: bool,
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

    /// Leave a channel (stop receiving events from it)
    LeaveChannel {
        /// Channel name
        channel: String,
    },

    /// Archive a channel
    ArchiveChannel {
        /// Channel name
        name: String,
    },

    /// Unarchive a channel
    UnarchiveChannel {
        /// Channel name
        name: String,
    },

    /// List archived channels
    ArchivedChannels,

    /// Archive a direct-message thread with a peer
    ArchiveDm {
        /// Peer handler
        peer: String,
    },

    /// Unarchive a direct-message thread with a peer
    UnarchiveDm {
        /// Peer handler
        peer: String,
    },

    /// List archived DMs the caller participates in
    ListArchivedDms,

    /// List handlers that have departed the workspace
    ListArchivedUsers,

    /// Self-burn: depart this clone's own handler from the workspace.
    /// Reads handler from local me.json — no parameters accepted.
    BurnSelf,

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
        /// Include card discussion messages in results
        #[arg(long)]
        include_cards: bool,
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
        /// Handler (git 必填; github 可选，配合 --display-name 替代 --token)
        #[arg(long)]
        handler: Option<String>,
        /// Display name (git 必填; github 可选，配合 --handler 替代 --token)
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

    /// Update GitIM to the latest version (or a specified version)
    Update {
        /// Target version (e.g. "0.4.0"). Defaults to latest release.
        version: Option<String>,
        /// Skip confirmation prompts
        #[arg(short, long)]
        yes: bool,
    },

    /// Card commands
    Card {
        #[command(subcommand)]
        command: CardCommands,
    },

    /// Board commands
    Board {
        #[command(subcommand)]
        command: BoardCommands,
    },
}

#[derive(Subcommand)]
enum DmCommands {
    /// Send a direct message
    Send {
        /// Target handler
        handler: String,
        /// Message body
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        body: Option<String>,
        /// Read message body from stdin
        #[arg(long)]
        stdin: bool,
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
enum CardCommands {
    /// Create a new card in a channel
    Create {
        /// Channel name
        channel: String,
        /// Card title
        title: String,
        /// Labels (repeatable)
        #[arg(short, long)]
        label: Vec<String>,
        /// Assignee handler
        #[arg(long)]
        assignee: Option<String>,
        /// Initial status (todo/doing/done)
        #[arg(long)]
        status: Option<String>,
    },

    /// List cards with optional filters
    Ls {
        /// Filter by channel
        #[arg(short, long)]
        channel: Option<String>,
        /// Filter by label (repeatable; all must match)
        #[arg(short, long)]
        label: Vec<String>,
        /// Filter by status
        #[arg(long)]
        status: Option<String>,
        /// Filter by assignee handler
        #[arg(long)]
        assignee: Option<String>,
    },

    /// Read card discussion
    Read {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// Maximum number of entries
        #[arg(short, long)]
        limit: Option<u64>,
        /// Only return entries after this line number
        #[arg(short, long)]
        since: Option<u64>,
    },

    /// Comment on a card
    Comment {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// Message body
        #[arg(required_unless_present = "stdin", conflicts_with = "stdin")]
        body: Option<String>,
        /// Read message body from stdin
        #[arg(long)]
        stdin: bool,
        /// Line number to reply to
        #[arg(short, long)]
        reply_to: Option<u64>,
    },

    /// Update card status / labels / assignee
    Update {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
        /// New status
        #[arg(long)]
        status: Option<String>,
        /// Replace labels (repeatable)
        #[arg(short, long)]
        label: Vec<String>,
        /// Clear labels (if set, ignore --label)
        #[arg(long)]
        label_clear: bool,
        /// New assignee handler
        #[arg(long)]
        assignee: Option<String>,
    },

    /// Archive a card
    Archive {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
    },

    /// Unarchive a card
    Unarchive {
        /// Channel name
        channel: String,
        /// Card ID
        card_id: String,
    },

    /// List archived cards
    Archived {
        /// Filter by channel name
        #[arg(short, long)]
        channel: Option<String>,
    },
}

#[derive(Subcommand)]
enum BoardCommands {
    /// Print the local path to your board file
    Path,

    /// Create your board
    Init,

    /// Show a handler's board
    Show {
        /// Handler whose board should be shown
        handler: String,
    },

    /// List valid boards
    Ls,

    /// Publish your board
    Publish {
        /// Read replacement board content from stdin
        #[arg(long)]
        stdin: bool,
    },

    /// Set a board frontmatter field
    Set {
        /// Field name: status, summary, or tags
        field: String,
        /// Field value
        value: String,
    },

    /// Edit board sections
    Section {
        #[command(subcommand)]
        command: BoardSectionCommands,
    },
}

#[derive(Subcommand)]
enum BoardSectionCommands {
    /// Replace a section with stdin content
    Set {
        /// Section heading
        section: String,
        /// Read replacement section content from stdin
        #[arg(long, required = true)]
        stdin: bool,
    },

    /// Append stdin content to a section
    Append {
        /// Section heading
        section: String,
        /// Read appended section content from stdin
        #[arg(long, required = true)]
        stdin: bool,
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

    if let Commands::Update { version, yes } = &cli.command {
        commands::update::cmd_update(version.as_deref(), *yes || cli.json).await;
        return;
    }

    if let Commands::Board {
        command: BoardCommands::Path,
    } = &cli.command
    {
        commands::board::cmd_path(&mode);
        return;
    }

    let client = init_client();

    match cli.command {
        Commands::Stop => unreachable!(),
        Commands::Onboard { .. } => unreachable!(),
        Commands::Update { .. } => unreachable!(),
        Commands::Board {
            command: BoardCommands::Path,
        } => unreachable!(),
        Commands::Status => cmd_status(&client, &mode).await,
        Commands::Send {
            channel,
            body,
            stdin,
            author,
            reply_to,
        } => {
            let body = read_body_or_exit(body, stdin);
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
        } => commands::messaging::cmd_read(&client, &mode, &channel, limit, since).await,
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
        Commands::LeaveChannel { channel } => {
            commands::channels::cmd_leave_channel(&client, &mode, &channel).await
        }
        Commands::ArchiveChannel { name } => {
            commands::channels::cmd_archive_channel(&client, &mode, &name).await
        }
        Commands::UnarchiveChannel { name } => {
            commands::channels::cmd_unarchive_channel(&client, &mode, &name).await
        }
        Commands::ArchivedChannels => {
            commands::channels::cmd_archived_channels(&client, &mode).await
        }
        Commands::ArchiveDm { peer } => commands::dm::cmd_archive_dm(&client, &mode, &peer).await,
        Commands::UnarchiveDm { peer } => {
            commands::dm::cmd_unarchive_dm(&client, &mode, &peer).await
        }
        Commands::ListArchivedDms => commands::dm::cmd_list_archived_dms(&client, &mode).await,
        Commands::ListArchivedUsers => commands::dm::cmd_list_archived_users(&client, &mode).await,
        Commands::BurnSelf => commands::burn::cmd_burn_self(&client, &mode).await,
        Commands::Users => commands::admin::cmd_users(&client, &mode).await,
        Commands::Search {
            query,
            author,
            channel,
            channel_type,
            limit,
            offset,
            include_cards,
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
                include_cards,
            )
            .await
        }
        Commands::Reindex => commands::admin::cmd_reindex(&client, &mode).await,
        Commands::Dm { command } => match command {
            DmCommands::Send {
                handler,
                body,
                stdin,
                author,
                reply_to,
            } => {
                let body = read_body_or_exit(body, stdin);
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
                commands::dm::cmd_dm_read(&client, &mode, &handler, author.as_deref(), limit, since)
                    .await
            }
            DmCommands::List => commands::dm::cmd_dm_list(&mode),
        },
        Commands::Card { command } => match command {
            CardCommands::Create {
                channel,
                title,
                label,
                assignee,
                status,
            } => {
                commands::card::cmd_create_card(
                    &client,
                    &mode,
                    &channel,
                    &title,
                    if label.is_empty() { None } else { Some(&label) },
                    assignee.as_deref(),
                    status.as_deref(),
                )
                .await
            }
            CardCommands::Ls {
                channel,
                label,
                status,
                assignee,
            } => {
                commands::card::cmd_list_cards(
                    &client,
                    &mode,
                    channel.as_deref(),
                    if label.is_empty() { None } else { Some(&label) },
                    status.as_deref(),
                    assignee.as_deref(),
                )
                .await
            }
            CardCommands::Read {
                channel,
                card_id,
                limit,
                since,
            } => {
                commands::card::cmd_read_card(&client, &mode, &channel, &card_id, limit, since)
                    .await
            }
            CardCommands::Comment {
                channel,
                card_id,
                body,
                stdin,
                reply_to,
            } => {
                let body = read_body_or_exit(body, stdin);
                commands::card::cmd_send_card_message(
                    &client, &mode, &channel, &card_id, &body, reply_to,
                )
                .await
            }
            CardCommands::Update {
                channel,
                card_id,
                status,
                label,
                label_clear,
                assignee,
            } => {
                let labels_param: Option<Vec<String>> = if label_clear {
                    Some(Vec::new())
                } else if !label.is_empty() {
                    Some(label)
                } else {
                    None
                };
                commands::card::cmd_update_card(
                    &client,
                    &mode,
                    &channel,
                    &card_id,
                    status.as_deref(),
                    labels_param.as_deref(),
                    assignee.as_deref(),
                )
                .await
            }
            CardCommands::Archive { channel, card_id } => {
                commands::card::cmd_archive_card(&client, &mode, &channel, &card_id).await
            }
            CardCommands::Unarchive { channel, card_id } => {
                commands::card::cmd_unarchive_card(&client, &mode, &channel, &card_id).await
            }
            CardCommands::Archived { channel } => {
                commands::card::cmd_archived_cards(&client, &mode, channel.as_deref()).await
            }
        },
        Commands::Board { command } => match command {
            BoardCommands::Path => unreachable!(),
            BoardCommands::Init => commands::board::cmd_init(&client, &mode).await,
            BoardCommands::Show { handler } => {
                commands::board::cmd_show(&client, &mode, &handler).await
            }
            BoardCommands::Ls => commands::board::cmd_ls(&client, &mode).await,
            BoardCommands::Publish { stdin } => {
                let content = if stdin {
                    Some(read_stdin_or_exit("failed to read board content"))
                } else {
                    None
                };
                commands::board::cmd_publish(&client, &mode, content.as_deref()).await
            }
            BoardCommands::Set { field, value } => {
                commands::board::cmd_set(&client, &mode, &field, &value).await
            }
            BoardCommands::Section { command } => match command {
                BoardSectionCommands::Set { section, stdin } => {
                    let _ = stdin;
                    let value = read_stdin_or_exit("failed to read section content");
                    commands::board::cmd_section_set(&client, &mode, &section, &value).await
                }
                BoardSectionCommands::Append { section, stdin } => {
                    let _ = stdin;
                    let value = read_stdin_or_exit("failed to read section content");
                    commands::board::cmd_section_append(&client, &mode, &section, &value).await
                }
            },
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

fn read_body_or_exit(body: Option<String>, stdin: bool) -> String {
    match (body, stdin) {
        (Some(body), false) => body,
        (None, true) => read_stdin_or_exit("failed to read stdin"),
        (Some(_), true) => {
            eprintln!("Error: cannot pass both a message body and --stdin");
            process::exit(1);
        }
        (None, false) => {
            eprintln!("Error: message body is required unless --stdin is set");
            process::exit(1);
        }
    }
}

fn read_stdin_or_exit(context: &str) -> String {
    let mut buf = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut buf) {
        eprintln!("Error: {context}: {e}");
        process::exit(1);
    }
    buf
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

#[cfg(test)]
mod tests {
    use super::Cli;
    use clap::Parser;

    #[test]
    fn send_accepts_stdin_without_body() {
        let parsed = Cli::try_parse_from(["gitim", "send", "general", "--stdin"]);

        assert!(
            parsed.is_ok(),
            "send should accept --stdin without a positional body: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn dm_send_accepts_stdin_without_body() {
        let parsed = Cli::try_parse_from(["gitim", "dm", "send", "alice", "--stdin"]);

        assert!(
            parsed.is_ok(),
            "dm send should accept --stdin without a positional body: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn card_comment_accepts_stdin_without_body() {
        let parsed = Cli::try_parse_from([
            "gitim",
            "card",
            "comment",
            "dev",
            "20260424-stdin",
            "--stdin",
        ]);

        assert!(
            parsed.is_ok(),
            "card comment should accept --stdin without a positional body: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn board_publish_accepts_stdin_without_content_argument() {
        let parsed = Cli::try_parse_from(["gitim", "board", "publish", "--stdin"]);

        assert!(
            parsed.is_ok(),
            "board publish should accept --stdin without a positional body: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn board_section_set_accepts_required_stdin() {
        let parsed =
            Cli::try_parse_from(["gitim", "board", "section", "set", "当前状态", "--stdin"]);

        assert!(
            parsed.is_ok(),
            "board section set should accept --stdin: {}",
            parsed.err().map(|e| e.to_string()).unwrap_or_default()
        );
    }

    #[test]
    fn board_section_append_rejects_missing_stdin() {
        let parsed = Cli::try_parse_from(["gitim", "board", "section", "append", "当前状态"]);

        assert!(parsed.is_err(), "section append should require --stdin");
    }
}
