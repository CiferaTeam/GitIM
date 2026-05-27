//! `gitim labels` subcommand — manage caller's own user-meta labels.
//!
//! Self-claim only: all add/remove targets are `self` (the daemon's bound
//! handler from me.json). Read APIs (`list`, `match`) accept any handler.

use clap::Subcommand;
use gitim_client::GitimClient;
use std::process;

use super::{get_repo_root, read_my_handler};
use crate::output::OutputMode;

#[derive(Subcommand)]
pub enum LabelsCommand {
    /// Add labels to your own user.meta.yaml
    Add {
        /// Labels to add (space-separated)
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
    /// Remove labels from your own user.meta.yaml
    Remove {
        /// Labels to remove (space-separated)
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
    /// List labels for a user (default: yourself)
    List {
        /// Target handler (default: self from me.json)
        #[arg(long)]
        handler: Option<String>,
    },
    /// Find agents matching ALL given labels (all-of intersection)
    Match {
        /// Labels to match against (space-separated)
        #[arg(required = true, num_args = 1..)]
        labels: Vec<String>,
    },
}

pub async fn run(client: &GitimClient, cmd: LabelsCommand, mode: OutputMode) {
    let repo_root = get_repo_root();
    let me = read_my_handler(&repo_root);

    match cmd {
        LabelsCommand::Add { labels } => match client.labels_add(&me, &labels).await {
            Ok(resp) => {
                if matches!(mode, OutputMode::Json) {
                    let j = serde_json::to_string(&resp).unwrap_or_default();
                    println!("{j}");
                } else {
                    println!("labels: [{}]", resp.current_labels.join(", "));
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        },
        LabelsCommand::Remove { labels } => match client.labels_remove(&me, &labels).await {
            Ok(resp) => {
                if matches!(mode, OutputMode::Json) {
                    let j = serde_json::to_string(&resp).unwrap_or_default();
                    println!("{j}");
                } else {
                    println!("labels: [{}]", resp.current_labels.join(", "));
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        },
        LabelsCommand::List { handler } => {
            let target = handler.unwrap_or(me);
            match client.labels_list(&target).await {
                Ok(resp) => {
                    if matches!(mode, OutputMode::Json) {
                        let j = serde_json::to_string(&resp).unwrap_or_default();
                        println!("{j}");
                    } else if resp.labels.is_empty() {
                        println!("@{}: (no labels)", resp.handler);
                    } else {
                        println!("@{}: [{}]", resp.handler, resp.labels.join(", "));
                    }
                }
                Err(e) => {
                    eprintln!("Error: {e}");
                    process::exit(1);
                }
            }
        }
        LabelsCommand::Match { labels } => match client.agents_with_labels(&labels).await {
            Ok(resp) => {
                if matches!(mode, OutputMode::Json) {
                    let j = serde_json::to_string(&resp).unwrap_or_default();
                    println!("{j}");
                } else if resp.handlers.is_empty() {
                    println!("(no agents match all of: {})", labels.join(", "));
                } else {
                    for h in &resp.handlers {
                        println!("@{h}");
                    }
                }
            }
            Err(e) => {
                eprintln!("Error: {e}");
                process::exit(1);
            }
        },
    }
}
