#![deny(warnings)]

use std::path::Path;
use std::{env, fs, process};

use gitim_client::{find_repo_root, GitimClient};

use crate::output::OutputMode;

/// Read the current user's handler from `.gitim/me.json`.
fn read_my_handler(repo_root: &Path) -> String {
    let me_path = repo_root.join(".gitim/me.json");
    let contents = match fs::read_to_string(&me_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Error: cannot read {}: {e}", me_path.display());
            process::exit(1);
        }
    };
    let v: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("Error: invalid me.json: {e}");
            process::exit(1);
        }
    };
    match v.get("handler").and_then(|h| h.as_str()) {
        Some(h) => h.to_string(),
        None => {
            eprintln!("Error: me.json missing \"handler\" field");
            process::exit(1);
        }
    }
}

/// Build the DM channel name from two handlers: `dm:{sorted[0]},{sorted[1]}`.
fn dm_channel(h1: &str, h2: &str) -> String {
    let (a, b) = if h1 < h2 { (h1, h2) } else { (h2, h1) };
    format!("dm:{a},{b}")
}

fn get_repo_root() -> std::path::PathBuf {
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

pub async fn cmd_dm_send(
    client: &GitimClient,
    mode: &OutputMode,
    target: &str,
    body: &str,
    author: Option<&str>,
    reply_to: Option<u64>,
) {
    let repo_root = get_repo_root();
    let my_handler = author
        .map(|a| a.to_string())
        .unwrap_or_else(|| read_my_handler(&repo_root));
    let channel = dm_channel(&my_handler, target);

    match client.send(&channel, body, Some(&my_handler), reply_to).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("DM sent."),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_dm_read(
    client: &GitimClient,
    mode: &OutputMode,
    target: &str,
    author: Option<&str>,
    limit: Option<u64>,
    since: Option<u64>,
) {
    let repo_root = get_repo_root();
    let my_handler = author
        .map(|a| a.to_string())
        .unwrap_or_else(|| read_my_handler(&repo_root));
    let channel = dm_channel(&my_handler, target);

    match client.read(&channel, limit, since).await {
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

pub fn cmd_dm_list(mode: &OutputMode) {
    let repo_root = get_repo_root();
    let dm_dir = repo_root.join("dm");

    let entries = match fs::read_dir(&dm_dir) {
        Ok(e) => e,
        Err(_) => {
            match mode {
                OutputMode::Human => println!("No DM conversations."),
                OutputMode::Json => println!("[]"),
            }
            return;
        }
    };

    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.strip_suffix(".thread").map(|s| s.to_string())
        })
        .collect();

    if names.is_empty() {
        match mode {
            OutputMode::Human => println!("No DM conversations."),
            OutputMode::Json => println!("[]"),
        }
        return;
    }

    names.sort();

    match mode {
        OutputMode::Human => {
            for name in &names {
                println!("{name}");
            }
        }
        OutputMode::Json => {
            let arr: Vec<serde_json::Value> = names
                .iter()
                .map(|n| serde_json::Value::String(n.clone()))
                .collect();
            match serde_json::to_string(&arr) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("Error: failed to format output: {e}");
                    process::exit(1);
                }
            }
        }
    }
}
