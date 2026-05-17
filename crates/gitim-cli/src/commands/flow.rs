#![deny(warnings)]

use std::process;

use gitim_client::GitimClient;

use crate::output::OutputMode;

fn print_json(value: serde_json::Value) {
    match serde_json::to_string(&value) {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("Error: failed to format output: {e}");
            process::exit(1);
        }
    }
}

fn print_or_exit(
    resp: gitim_client::ApiResponse,
    mode: &OutputMode,
    human_success: impl FnOnce(&serde_json::Value),
) {
    if !resp.ok {
        eprintln!("Error: {}", resp.error.as_deref().unwrap_or("unknown"));
        process::exit(1);
    }

    let data = resp.data.unwrap_or(serde_json::Value::Null);
    match mode {
        OutputMode::Human => human_success(&data),
        OutputMode::Json => print_json(data),
    }
}

pub async fn cmd_flow_list(client: &GitimClient, mode: &OutputMode) {
    match client.flow_list().await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let flows = data["flows"].as_array().cloned().unwrap_or_default();
            if flows.is_empty() {
                println!("(no flows)");
                return;
            }
            for f in flows {
                println!(
                    "  {:<20} {:<30} ({} nodes)",
                    f["slug"].as_str().unwrap_or(""),
                    f["name"].as_str().unwrap_or(""),
                    f["node_count"].as_u64().unwrap_or(0),
                );
                if let Some(desc) = f["description"].as_str() {
                    if !desc.is_empty() {
                        println!("    {}", desc);
                    }
                }
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_show(client: &GitimClient, mode: &OutputMode, slug: &str) {
    match client.flow_show(slug).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            println!(
                "# {} ({})",
                data["name"].as_str().unwrap_or(""),
                data["slug"].as_str().unwrap_or("")
            );
            if let Some(d) = data["description"].as_str() {
                if !d.is_empty() {
                    println!("{}\n", d);
                }
            }
            println!("---");
            println!("DAG:");
            let nodes = data["nodes"].as_array().cloned().unwrap_or_default();
            for n in &nodes {
                let id = n["id"].as_str().unwrap_or("");
                let needs: Vec<String> = n["needs"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if needs.is_empty() {
                    println!("  o {}", id);
                } else {
                    println!("  -> {}  (needs: {})", id, needs.join(", "));
                }
            }
            println!("---");
            if let Some(md) = data["raw_markdown"].as_str() {
                if !md.is_empty() {
                    println!("Raw markdown:\n{}", md);
                }
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_create(
    client: &GitimClient,
    mode: &OutputMode,
    slug: &str,
    name: &str,
    description: &str,
) {
    match client.flow_create(slug, name, description).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let slug_out = data["slug"].as_str().unwrap_or(slug);
            let path = data["path"].as_str().unwrap_or("");
            let commit = data["commit_id"].as_str().unwrap_or("");
            let commit_short = if commit.len() >= 8 {
                &commit[..8]
            } else {
                commit
            };
            println!(
                "已创建 flow `{}` (0 个节点)\n路径: {}\ncommit: {}\n下一步: 编辑 flows/{}/index.md 加节点",
                slug_out, path, commit_short, slug_out,
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_remove(client: &GitimClient, mode: &OutputMode, slug: &str) {
    match client.flow_remove(slug).await {
        Ok(resp) => print_or_exit(resp, mode, |_data| {
            println!("已删除 flow `{}` (移至 .trash/)", slug);
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_run_start(
    client: &GitimClient,
    mode: &OutputMode,
    slug: &str,
    channel: &str,
) {
    match client.flow_run_start(slug, channel).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            println!(
                "已启动 flow run `{}` (flow={}, channel={})\ncommit: {}",
                data["run_id"].as_str().unwrap_or(""),
                data["flow_slug"].as_str().unwrap_or(""),
                data["channel"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_runs(
    client: &GitimClient,
    mode: &OutputMode,
    slug: Option<&str>,
    channel: Option<&str>,
    status: Option<&str>,
) {
    match client.flow_run_list(slug, channel, status).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let runs = data["runs"].as_array().cloned().unwrap_or_default();
            if runs.is_empty() {
                println!("(no runs)");
                return;
            }
            for r in runs {
                println!(
                    "  {} {} [{:<11}] {}/{} nodes (channel={}, started_by={})",
                    r["run_id"].as_str().unwrap_or(""),
                    r["flow_slug"].as_str().unwrap_or(""),
                    r["status"].as_str().unwrap_or(""),
                    r["nodes_done"].as_u64().unwrap_or(0),
                    r["node_count"].as_u64().unwrap_or(0),
                    r["channel"].as_str().unwrap_or(""),
                    r["started_by"].as_str().unwrap_or(""),
                );
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_run_show(client: &GitimClient, mode: &OutputMode, run_id: &str) {
    match client.flow_run_show(run_id).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            println!(
                "run `{}` ({})\nflow: {}  channel: {}  by: {}\nstarted: {}  updated: {}\n",
                data["run_id"].as_str().unwrap_or(""),
                data["status"].as_str().unwrap_or(""),
                data["flow_slug"].as_str().unwrap_or(""),
                data["channel"].as_str().unwrap_or(""),
                data["started_by"].as_str().unwrap_or(""),
                data["started_at"].as_str().unwrap_or(""),
                data["updated_at"].as_str().unwrap_or(""),
            );
            println!("Nodes:");
            for n in data["nodes"].as_array().cloned().unwrap_or_default() {
                let id = n["id"].as_str().unwrap_or("");
                let st = n["status"].as_str().unwrap_or("");
                let actor = n["actor"].as_str().unwrap_or("-");
                let marker = match st {
                    "done" => "o",
                    "in_progress" => ">",
                    "pending" => ".",
                    "failed" => "x",
                    "skipped" => "~",
                    _ => "?",
                };
                println!("  {} [{:<11}] {}  @{}", marker, st, id, actor);
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_node_set(
    client: &GitimClient,
    mode: &OutputMode,
    run_id: &str,
    node_id: &str,
    status: &str,
    actor: Option<&str>,
    result_ref: Option<&str>,
) {
    match client
        .flow_node_set(run_id, node_id, status, actor, result_ref)
        .await
    {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            println!(
                "已更新 node `{}` → {} (run={}, run_status={})\ncommit: {}",
                data["node_id"].as_str().unwrap_or(""),
                data["status"].as_str().unwrap_or(""),
                data["run_id"].as_str().unwrap_or(""),
                data["run_status"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_run_cancel(client: &GitimClient, mode: &OutputMode, run_id: &str) {
    match client.flow_run_cancel(run_id).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            println!(
                "已取消 flow run `{}`\ncommit: {}",
                data["run_id"].as_str().unwrap_or(""),
                data["commit_id"].as_str().unwrap_or(""),
            );
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_flow_validate(client: &GitimClient, mode: &OutputMode, slug: &str) {
    match client.flow_validate(slug).await {
        Ok(resp) => print_or_exit(resp, mode, |data| {
            let slug_out = data["slug"].as_str().unwrap_or(slug);
            let ok = data["ok"].as_bool().unwrap_or(false);
            println!("flow `{}`: {}", slug_out, if ok { "OK" } else { "FAIL" });
            let items = data["items"].as_array().cloned().unwrap_or_default();
            for it in items {
                let kind = it["kind"].as_str().unwrap_or("");
                let msg = it["message"].as_str().unwrap_or("");
                let marker = if kind == "error" { "x" } else { "!" };
                println!("  {} [{}] {}", marker, kind, msg);
            }
            if !ok {
                process::exit(2);
            }
        }),
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}
