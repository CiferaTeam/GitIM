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
        #[arg(
            short,
            long,
            help = "Maximum number of messages to return",
            long_help = "Maximum number of messages to return.\n\
                         \n\
                         Alone: the last N messages in the channel.\n\
                         With --since: the first N messages after the cursor."
        )]
        limit: Option<u64>,
        #[arg(
            short,
            long,
            help = "Return messages with line_number > N (a cursor, not a count)",
            long_help = "Return messages with line_number > N. SINCE is a cursor, \
                         not a count.\n\
                         \n\
                         Alone:        every message after the cursor.\n\
                         With --limit: the first LIMIT messages after the cursor.\n\
                         \n\
                         Page back through history: since = oldest_seen - limit - 1.\n\
                         Incremental poll:          since = last_seen_line."
        )]
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

    /// Cron trigger commands
    Cron {
        #[command(subcommand)]
        command: CronCommands,
    },

    /// Flow template commands
    Flow {
        #[command(subcommand)]
        command: FlowCommands,
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
        #[arg(
            short,
            long,
            help = "Maximum number of messages to return",
            long_help = "Maximum number of messages to return.\n\
                         \n\
                         Alone: the last N messages.\n\
                         With --since: the first N after the cursor."
        )]
        limit: Option<u64>,
        #[arg(
            short,
            long,
            help = "Return messages with line_number > N (a cursor, not a count)",
            long_help = "Return messages with line_number > N. SINCE is a cursor, \
                         not a count.\n\
                         \n\
                         With --limit: the first LIMIT messages after the cursor.\n\
                         Use to page back history (since = oldest_seen - limit - 1) \
                         or to pull incrementally from a known cursor."
        )]
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
        #[arg(
            short,
            long,
            help = "Maximum number of entries",
            long_help = "Maximum number of entries.\n\
                         \n\
                         Alone: the last N entries.\n\
                         With --since: the first N after the cursor."
        )]
        limit: Option<u64>,
        #[arg(
            short,
            long,
            help = "Return entries with line_number > N (a cursor, not a count)",
            long_help = "Return entries with line_number > N. SINCE is a cursor, \
                         not a count.\n\
                         \n\
                         With --limit: the first LIMIT entries after the cursor.\n\
                         Use to page back history (since = oldest_seen - limit - 1) \
                         or to pull incrementally from a known cursor."
        )]
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

#[derive(Subcommand)]
enum CronCommands {
    /// Create a new cron trigger.
    ///
    /// Provide exactly one of `--prompt` (inline) or `--prompt-file`
    /// (read from disk; useful for multi-line prompts that are awkward
    /// to shell-quote).
    Create {
        /// Cron job name (lowercase a-z 0-9 hyphen, 1–63 chars)
        name: String,
        /// 5-field cron expression or alias (`@daily`, `@weekly`, ...)
        #[arg(long)]
        schedule: String,
        /// Target handler. Accepts `@self`, `@bob`, or bare `bob`.
        #[arg(long)]
        target: String,
        /// Inline prompt body. Mutually exclusive with --prompt-file.
        #[arg(
            long,
            conflicts_with = "prompt_file",
            required_unless_present = "prompt_file"
        )]
        prompt: Option<String>,
        /// Path to a UTF-8 prompt file. Mutually exclusive with --prompt.
        #[arg(long, conflicts_with = "prompt", required_unless_present = "prompt")]
        prompt_file: Option<std::path::PathBuf>,
        /// IANA timezone (e.g. `America/Los_Angeles`); defaults to UTC.
        #[arg(long)]
        timezone: Option<String>,
    },

    /// List all active cron triggers
    List,

    /// Show full spec + recent runs + next fire for a single cron
    Show {
        /// Cron job name
        name: String,
    },

    /// List past fires (newest first) for a cron
    History {
        /// Cron job name
        name: String,
        /// Maximum number of past fires to return (default 50, max 1000)
        #[arg(long)]
        limit: Option<u32>,
    },

    /// Pause a cron (keeps spec, suppresses fires)
    Disable {
        /// Cron job name
        name: String,
    },

    /// Resume a paused cron
    Enable {
        /// Cron job name
        name: String,
    },

    /// Soft-delete: move spec + history into archive/crons/
    Delete {
        /// Cron job name
        name: String,
    },

    /// Print the next fire timestamp (ISO 8601 UTC) on a single line
    Next {
        /// Cron job name
        name: String,
    },
}

#[derive(Subcommand)]
enum FlowCommands {
    /// List all flow templates
    List,
    /// Show a flow template (markdown + ascii DAG)
    Show { slug: String },
    /// Create a stub flow template
    Create {
        slug: String,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        description: String,
    },
    /// Soft-delete a flow template (move to .trash/)
    Rm { slug: String },
    /// Validate a flow template (schema + double-source alignment)
    Validate { slug: String },
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
        Commands::Cron { command } => match command {
            CronCommands::Create {
                name,
                schedule,
                target,
                prompt,
                prompt_file,
                timezone,
            } => {
                let prompt_body =
                    match commands::cron::load_prompt(prompt.as_deref(), prompt_file.as_deref()) {
                        Ok(s) => s,
                        Err(e) => {
                            eprintln!("Error: {e}");
                            process::exit(1);
                        }
                    };
                commands::cron::cmd_create(
                    &client,
                    &mode,
                    &name,
                    &schedule,
                    &target,
                    timezone.as_deref(),
                    &prompt_body,
                )
                .await
            }
            CronCommands::List => commands::cron::cmd_list(&client, &mode).await,
            CronCommands::Show { name } => commands::cron::cmd_show(&client, &mode, &name).await,
            CronCommands::History { name, limit } => {
                commands::cron::cmd_history(&client, &mode, &name, limit).await
            }
            CronCommands::Disable { name } => {
                commands::cron::cmd_disable(&client, &mode, &name).await
            }
            CronCommands::Enable { name } => {
                commands::cron::cmd_enable(&client, &mode, &name).await
            }
            CronCommands::Delete { name } => {
                commands::cron::cmd_delete(&client, &mode, &name).await
            }
            CronCommands::Next { name } => commands::cron::cmd_next(&client, &mode, &name).await,
        },
        Commands::Flow { command } => match command {
            FlowCommands::List => commands::flow::cmd_flow_list(&client, &mode).await,
            FlowCommands::Show { slug } => {
                commands::flow::cmd_flow_show(&client, &mode, &slug).await
            }
            FlowCommands::Create {
                slug,
                name,
                description,
            } => commands::flow::cmd_flow_create(&client, &mode, &slug, &name, &description).await,
            FlowCommands::Rm { slug } => {
                commands::flow::cmd_flow_remove(&client, &mode, &slug).await
            }
            FlowCommands::Validate { slug } => {
                commands::flow::cmd_flow_validate(&client, &mode, &slug).await
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

    // -- cron subcommand parsing -----------------------------------------
    //
    // These lock the clap surface: required flags, mutually-exclusive
    // pairs, and the canonical syntax the agent prompt templates tell
    // agents to type. Drift in any of these would either break the docs
    // or bury an error behind clap's generic "unexpected argument".

    #[test]
    fn cron_create_with_inline_prompt_parses() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "weekly-report",
            "--schedule",
            "0 9 * * 1",
            "--target",
            "@self",
            "--prompt",
            "summarize",
        ]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn cron_create_with_prompt_file_parses() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "weekly-report",
            "--schedule",
            "@weekly",
            "--target",
            "alice",
            "--prompt-file",
            "/tmp/prompt.txt",
        ]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn cron_create_with_timezone_parses() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "daily-standup",
            "--schedule",
            "0 9 * * *",
            "--target",
            "@self",
            "--prompt",
            "standup",
            "--timezone",
            "America/Los_Angeles",
        ]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    /// Both --prompt and --prompt-file → clap reports conflict (exit 2).
    /// The point: clap, not custom code, enforces mutual exclusion.
    #[test]
    fn cron_create_rejects_both_prompt_and_prompt_file() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "x",
            "--schedule",
            "@daily",
            "--target",
            "@self",
            "--prompt",
            "inline",
            "--prompt-file",
            "/tmp/p",
        ]);
        // Cli doesn't derive Debug; collapse Ok(_) to a sentinel error so
        // we can still assert via `match`.
        let err = match r {
            Ok(_) => panic!("expected clap to reject conflicting prompt flags"),
            Err(e) => e,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    /// Neither --prompt nor --prompt-file → clap reports required (exit 2).
    #[test]
    fn cron_create_rejects_missing_prompt_source() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "x",
            "--schedule",
            "@daily",
            "--target",
            "@self",
        ]);
        let err = match r {
            Ok(_) => panic!("expected clap to require --prompt or --prompt-file"),
            Err(e) => e,
        };
        // Clap reports MissingRequiredArgument when required_unless_present
        // chains both come up empty.
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn cron_create_rejects_missing_schedule() {
        let r = Cli::try_parse_from([
            "gitim", "cron", "create", "x", "--target", "@self", "--prompt", "p",
        ]);
        assert!(r.is_err());
    }

    #[test]
    fn cron_create_rejects_missing_target() {
        let r = Cli::try_parse_from([
            "gitim",
            "cron",
            "create",
            "x",
            "--schedule",
            "@daily",
            "--prompt",
            "p",
        ]);
        assert!(r.is_err());
    }

    #[test]
    fn cron_list_parses() {
        let r = Cli::try_parse_from(["gitim", "cron", "list"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_list_with_global_json_flag_parses() {
        // --json is global, so it works on cron list too.
        let r = Cli::try_parse_from(["gitim", "--json", "cron", "list"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_show_parses() {
        let r = Cli::try_parse_from(["gitim", "cron", "show", "weekly-report"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_history_parses_with_limit() {
        let r = Cli::try_parse_from(["gitim", "cron", "history", "weekly-report", "--limit", "10"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_history_parses_without_limit() {
        let r = Cli::try_parse_from(["gitim", "cron", "history", "weekly-report"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_disable_enable_delete_parse() {
        for sub in ["disable", "enable", "delete"] {
            let r = Cli::try_parse_from(["gitim", "cron", sub, "weekly-report"]);
            assert!(r.is_ok(), "{sub} failed to parse");
        }
    }

    #[test]
    fn cron_next_parses() {
        let r = Cli::try_parse_from(["gitim", "cron", "next", "weekly-report"]);
        assert!(r.is_ok());
    }

    #[test]
    fn cron_subcommand_requires_a_subcommand() {
        let r = Cli::try_parse_from(["gitim", "cron"]);
        assert!(r.is_err());
    }

    // -- flow subcommand parsing --------------------------------------------

    #[test]
    fn flow_list_parses() {
        let r = Cli::try_parse_from(["gitim", "flow", "list"]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_show_parses() {
        let r = Cli::try_parse_from(["gitim", "flow", "show", "release"]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_create_parses() {
        let r = Cli::try_parse_from(["gitim", "flow", "create", "release", "--name", "Release"]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_create_with_description_parses() {
        let r = Cli::try_parse_from([
            "gitim",
            "flow",
            "create",
            "release",
            "--name",
            "Release",
            "--description",
            "monthly release flow",
        ]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_rm_parses() {
        let r = Cli::try_parse_from(["gitim", "flow", "rm", "release"]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_validate_parses() {
        let r = Cli::try_parse_from(["gitim", "flow", "validate", "release"]);
        assert!(r.is_ok(), "{:?}", r.err().map(|e| e.to_string()));
    }

    #[test]
    fn flow_subcommand_requires_a_subcommand() {
        let r = Cli::try_parse_from(["gitim", "flow"]);
        assert!(r.is_err());
    }

    #[test]
    fn flow_create_requires_name() {
        let r = Cli::try_parse_from(["gitim", "flow", "create", "release"]);
        assert!(r.is_err());
    }
}
