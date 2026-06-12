use std::process;

use gitim_client::GitimClient;
use gitim_core::responses::ListProjectsResponse;
use serde_json::Value;

use crate::output::OutputMode;

pub async fn cmd_list_projects(client: &GitimClient, mode: &OutputMode) {
    match client.list_projects().await {
        Ok(resp) => {
            if !resp.ok {
                let code = resp.error_code.as_deref().unwrap_or("");
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error ({code}): {msg}");
                process::exit(1);
            }
            let data = resp.data.unwrap_or(Value::Null);
            match mode {
                OutputMode::Json => match serde_json::to_string(&data) {
                    Ok(s) => println!("{s}"),
                    Err(e) => {
                        eprintln!("Error: format output: {e}");
                        process::exit(1);
                    }
                },
                OutputMode::Human => {
                    let parsed: ListProjectsResponse = match serde_json::from_value(data) {
                        Ok(p) => p,
                        Err(e) => {
                            eprintln!("Error: parse response: {e}");
                            process::exit(1);
                        }
                    };
                    if parsed.projects.is_empty() {
                        println!("(no projects)");
                        return;
                    }
                    for p in &parsed.projects {
                        println!(
                            "{:<24}  {} channel(s)  — {}",
                            p.slug, p.channel_count, p.meta.display_name
                        );
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

pub async fn cmd_create_project(
    client: &GitimClient,
    mode: &OutputMode,
    slug: &str,
    display_name: &str,
    introduction: &str,
) {
    match client
        .create_project(slug, display_name, introduction)
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let code = resp.error_code.as_deref().unwrap_or("");
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error ({code}): {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("Project '{slug}' created"),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: {e}");
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
