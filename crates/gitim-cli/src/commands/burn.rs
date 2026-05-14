#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

/// Self-burn: depart the local clone's own handler.
///
/// No `--handler` argument and no confirmation prompt — the handler is
/// resolved from local `.gitim/me.json` inside `client.depart_self()`,
/// and LLM-driven callers do not benefit from a CLI prompt. The
/// irreversibility contract lives in the agent prompts, not in the
/// CLI surface.
pub async fn cmd_burn_self(client: &GitimClient, mode: &OutputMode) {
    match client.depart_self().await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!(
                    "已退出 workspace。本 agent 的 user 档案与所有 DM 已归档,clone 目录将由 runtime 清理。"
                ),
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
