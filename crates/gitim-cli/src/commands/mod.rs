pub mod admin;
pub mod board;
pub mod burn;
pub mod card;
pub mod channels;
pub mod cron;
pub mod dm;
pub mod flow;
pub mod messaging;
pub mod onboard;
pub mod timer;
pub mod update;

use gitim_client::find_repo_root;
use std::path::Path;
use std::{env, fs, process};

pub(super) fn get_repo_root() -> std::path::PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|e| {
        eprintln!("Error: cannot read current directory: {e}");
        process::exit(1);
    });
    match find_repo_root(&cwd) {
        Some(r) => r,
        None => {
            eprintln!("Error: not in a GitIM repository (no .gitim/ found)");
            process::exit(1);
        }
    }
}

pub(super) fn normalize_channel_arg(channel: &str) -> &str {
    channel
        .strip_prefix('#')
        .filter(|name| !name.is_empty())
        .unwrap_or(channel)
}

pub(super) fn read_my_handler(repo_root: &Path) -> String {
    let me_path = repo_root.join(".gitim/me.json");
    let contents = match fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: cannot read {}: {e}", me_path.display());
            process::exit(1);
        }
    };
    let me: gitim_core::me_json::MeJson = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: invalid me.json: {e}");
            process::exit(1);
        }
    };
    me.handler.unwrap_or_else(|| {
        eprintln!("Error: me.json missing \"handler\" field");
        process::exit(1);
    })
}
