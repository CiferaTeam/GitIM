#![deny(warnings)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{self, Command, Stdio};
use std::thread;
use std::time::Duration;

use regex::Regex;
use serde_json::json;

use gitim_client::{ensure_daemon, is_daemon_running, GitimClient};
use gitim_core::auth_payload::AuthPayload;

/// Git server type for onboarding
#[derive(Clone, Debug)]
pub enum GitServer {
    Git,
    Github,
    Gitea,
    Gitlab,
}

impl GitServer {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "git" => Some(Self::Git),
            "github" => Some(Self::Github),
            "gitea" => Some(Self::Gitea),
            "gitlab" => Some(Self::Gitlab),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Git => "git",
            Self::Github => "github",
            Self::Gitea => "gitea",
            Self::Gitlab => "gitlab",
        }
    }
}

pub struct OnboardArgs {
    pub repo_name: Option<String>,
    pub org: Option<String>,
    pub git_server: String,
    pub token: Option<String>,
    pub handler: Option<String>,
    pub display_name: Option<String>,
    pub url: Option<String>,
    pub refresh: bool,
    pub debug_http: bool,
    pub admin: bool,
    pub guest: bool,
}

fn validate_params(git_server: &GitServer, args: &OnboardArgs) {
    match git_server {
        GitServer::Git => {
            if args.handler.is_none() {
                eprintln!("Error: git 本地模式需要 --handler");
                process::exit(1);
            }
            if args.display_name.is_none() {
                eprintln!("Error: git 本地模式需要 --display-name");
                process::exit(1);
            }
        }
        GitServer::Github => {
            let has_handler = args.handler.is_some() && args.display_name.is_some();
            let has_token = args.token.is_some();
            if !has_handler && !has_token {
                eprintln!("Error: github 模式需要 --handler + --display-name 或 --token");
                process::exit(1);
            }
        }
        other => {
            let name = other.as_str();
            if args.token.is_none() {
                eprintln!("Error: {name} 模式需要 --token");
                process::exit(1);
            }
            if matches!(other, GitServer::Gitea | GitServer::Gitlab) && args.url.is_none() {
                eprintln!("Error: {name} 模式需要 --url（服务地址）");
                process::exit(1);
            }
        }
    }
}

fn build_auth(git_server: &GitServer, args: &OnboardArgs) -> AuthPayload {
    // If handler + display_name are provided, use Git-style auth
    // (works for both git and github modes with shared credentials)
    if let (Some(handler), Some(display_name)) = (&args.handler, &args.display_name) {
        return AuthPayload::Git {
            handler: handler.clone(),
            display_name: display_name.clone(),
            github_email: None,
        };
    }

    match git_server {
        GitServer::Git => {
            // validate_params guarantees handler+display_name for git mode
            AuthPayload::Git {
                handler: args.handler.as_ref().unwrap().clone(),
                display_name: args.display_name.as_ref().unwrap().clone(),
                github_email: None,
            }
        }
        GitServer::Github => AuthPayload::GitHub {
            token: args.token.as_ref().unwrap().clone(),
        },
        GitServer::Gitea => AuthPayload::Gitea {
            token: args.token.as_ref().unwrap().clone(),
            url: args.url.as_ref().unwrap().clone(),
        },
        GitServer::Gitlab => AuthPayload::GitLab {
            token: args.token.as_ref().unwrap().clone(),
            url: args.url.as_ref().unwrap().clone(),
        },
    }
}

fn clone_or_create_repo(
    repo_name: &str,
    org: Option<&str>,
    git_server: &GitServer,
    args: &OnboardArgs,
) -> PathBuf {
    let target_dir = env::current_dir()
        .unwrap_or_else(|e| {
            eprintln!("Error: cannot read current directory: {e}");
            process::exit(1);
        })
        .join(repo_name);

    match git_server {
        GitServer::Git => {
            fs::create_dir_all(&target_dir).unwrap_or_else(|e| {
                eprintln!("Error: cannot create directory: {e}");
                process::exit(1);
            });
            let status = Command::new("git")
                .args(["init"])
                .current_dir(&target_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();
            match status {
                Ok(s) if s.success() => {}
                _ => {
                    eprintln!("Error: git init 失败");
                    process::exit(1);
                }
            }
            target_dir
        }

        GitServer::Github => {
            let gh_target = match org {
                Some(o) => format!("{o}/{repo_name}"),
                None => repo_name.to_string(),
            };

            // Try gh CLI first (uses gh's own auth)
            let clone_ok = Command::new("gh")
                .args([
                    "repo",
                    "clone",
                    &gh_target,
                    target_dir.to_str().unwrap_or(repo_name),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if clone_ok {
                return target_dir;
            }

            // gh clone failed — try gh create
            let parent = target_dir.parent().unwrap_or_else(|| Path::new("."));
            let create_ok = Command::new("gh")
                .args(["repo", "create", &gh_target, "--private", "--clone"])
                .current_dir(parent)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if create_ok {
                return target_dir;
            }

            // gh not available or failed — fallback to git clone
            let org_name = org.unwrap_or_else(|| {
                eprintln!("Error: gh 不可用时，github 模式需要指定 org");
                eprintln!("  → 用法: gitim onboard <repo> <org> --git-server github --handler ...");
                process::exit(1);
            });

            let clone_url = if let Some(token) = &args.token {
                format!("https://x-access-token:{token}@github.com/{org_name}/{repo_name}.git")
            } else {
                format!("git@github.com:{org_name}/{repo_name}.git")
            };

            let git_clone_ok = Command::new("git")
                .args([
                    "clone",
                    &clone_url,
                    target_dir.to_str().unwrap_or(repo_name),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if git_clone_ok {
                return target_dir;
            }

            // All clone attempts failed — init locally + add remote
            eprintln!("Warning: 无法克隆远程仓库，创建本地 git 仓库");
            fs::create_dir_all(&target_dir).unwrap_or_else(|e| {
                eprintln!("Error: cannot create directory: {e}");
                process::exit(1);
            });
            let init_ok = Command::new("git")
                .args(["init"])
                .current_dir(&target_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !init_ok {
                eprintln!("Error: git init 失败");
                process::exit(1);
            }

            let remote_url = if let Some(token) = &args.token {
                format!("https://x-access-token:{token}@github.com/{org_name}/{repo_name}.git")
            } else {
                format!("git@github.com:{org_name}/{repo_name}.git")
            };
            let _ = Command::new("git")
                .args(["remote", "add", "origin", &remote_url])
                .current_dir(&target_dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status();

            target_dir
        }

        GitServer::Gitea | GitServer::Gitlab => {
            let server_name = git_server.as_str();
            let org = org.unwrap_or_else(|| {
                eprintln!("Error: {server_name} 模式需要指定 org（作为 URL 中的 owner）");
                eprintln!("  → 用法: gitim onboard <repo> <org> --git-server gitea --url ...");
                process::exit(1);
            });

            let base_url = args.url.as_deref().unwrap();
            let repo_url = format!("{base_url}/{org}/{repo_name}.git");

            // Try clone
            let clone_ok = Command::new("git")
                .args(["clone", &repo_url, target_dir.to_str().unwrap_or(repo_name)])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);

            if !clone_ok {
                if matches!(git_server, GitServer::Gitlab) {
                    eprintln!("Error: GitLab 不支持自动创建仓库，请先在 GitLab 上手动创建");
                    eprintln!(
                        "  → 创建后再运行: gitim onboard {repo_name} {org} --git-server gitlab --url {base_url} --token ..."
                    );
                    process::exit(1);
                }

                // Gitea: create via API then clone
                let token = args.token.as_deref().unwrap();
                let create_url = format!("{base_url}/api/v1/orgs/{org}/repos");
                let body =
                    serde_json::to_string(&json!({"name": repo_name, "private": true})).unwrap();

                let api_ok = Command::new("curl")
                    .args([
                        "-sf",
                        "-X",
                        "POST",
                        "-H",
                        &format!("Authorization: token {token}"),
                        "-H",
                        "Content-Type: application/json",
                        "-d",
                        &body,
                        &create_url,
                    ])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if !api_ok {
                    eprintln!("Error: 无法创建 Gitea 仓库 {repo_name}");
                    process::exit(1);
                }

                let clone2_ok = Command::new("git")
                    .args(["clone", &repo_url, target_dir.to_str().unwrap_or(repo_name)])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .map(|s| s.success())
                    .unwrap_or(false);

                if !clone2_ok {
                    eprintln!("Error: 无法创建 Gitea 仓库 {repo_name}");
                    process::exit(1);
                }
            }

            target_dir
        }
    }
}

fn ensure_config_debug_http(repo_dir: &Path, enabled: bool) {
    let config_path = repo_dir.join(".gitim/config.yaml");
    let value = enabled.to_string();

    if config_path.exists() {
        let mut content = fs::read_to_string(&config_path).unwrap_or_default();
        if content.contains("debug_http:") {
            // Replace existing value using regex to handle variable whitespace
            let re = Regex::new(r"debug_http:\s*(true|false)").unwrap();
            content = re
                .replace(&content, format!("debug_http: {value}"))
                .to_string();
        } else if content.contains("daemon:") {
            // Replace only the first occurrence of "daemon:"
            content = content.replacen("daemon:", &format!("daemon:\n  debug_http: {value}"), 1);
        } else {
            content.push_str(&format!("\ndaemon:\n  debug_http: {value}\n"));
        }
        fs::write(&config_path, content).unwrap_or_else(|e| {
            eprintln!("Error: cannot write config: {e}");
            process::exit(1);
        });
    } else {
        let gitim_dir = repo_dir.join(".gitim");
        fs::create_dir_all(&gitim_dir).unwrap_or_else(|e| {
            eprintln!("Error: cannot create .gitim directory: {e}");
            process::exit(1);
        });
        let content = format!("version: 1\ndaemon:\n  debug_http: {value}\n");
        fs::write(&config_path, content).unwrap_or_else(|e| {
            eprintln!("Error: cannot write config: {e}");
            process::exit(1);
        });
    }
}

pub async fn cmd_onboard(args: OnboardArgs) {
    let git_server = GitServer::from_str(&args.git_server).unwrap_or_else(|| {
        eprintln!("Error: unknown git server type: {}", args.git_server);
        process::exit(1);
    });

    // --guest and --admin are mutually exclusive
    if args.guest && args.admin {
        eprintln!("Error: --guest 和 --admin 不能同时使用");
        process::exit(1);
    }

    // --refresh mode
    if args.refresh {
        if !args.guest {
            validate_params(&git_server, &args);
        }

        let cwd = env::current_dir().unwrap_or_else(|e| {
            eprintln!("Error: cannot read current directory: {e}");
            process::exit(1);
        });

        let gitim_dir = cwd.join(".gitim");
        if !gitim_dir.exists() {
            eprintln!("不在 GitIM 仓库中，无法 --refresh");
            process::exit(1);
        }

        if args.debug_http {
            ensure_config_debug_http(&cwd, true);
            if is_daemon_running(&cwd) {
                let old_client = GitimClient::new(&cwd);
                let _ = old_client.stop().await;
                thread::sleep(Duration::from_millis(300));
            }
        }

        if let Err(e) = ensure_daemon(&cwd) {
            eprintln!("Error: failed to start daemon: {e}");
            process::exit(1);
        }

        let client = GitimClient::new(&cwd);
        let auth = if args.guest {
            None
        } else {
            Some(build_auth(&git_server, &args))
        };

        match client
            .onboard(git_server.as_str(), auth, args.admin, args.guest, true)
            .await
        {
            Ok(resp) => {
                if !resp.ok {
                    let msg = resp.error.as_deref().unwrap_or("unknown error");
                    eprintln!("身份刷新失败：{msg}");
                    process::exit(1);
                }
                if args.guest {
                    println!("游客模式已刷新");
                } else {
                    let handler = resp
                        .data
                        .as_ref()
                        .and_then(|d| d.get("handler"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("(unknown)");
                    let admin_tag = if args.admin { " [ADMIN]" } else { "" };
                    println!("身份已刷新：@{handler}{admin_tag}");
                }
            }
            Err(e) => {
                eprintln!("身份刷新失败：{e}");
                process::exit(1);
            }
        }
        return;
    }

    // Fresh onboard flow
    let repo_name = args.repo_name.as_deref().unwrap_or_else(|| {
        eprintln!("请指定仓库名称: gitim onboard <repo_name> [org]");
        process::exit(1);
    });

    if !args.guest {
        validate_params(&git_server, &args);
    }

    let repo_dir = clone_or_create_repo(repo_name, args.org.as_deref(), &git_server, &args);

    // Ensure .gitim/ directory
    let gitim_dir = repo_dir.join(".gitim");
    fs::create_dir_all(&gitim_dir).unwrap_or_else(|e| {
        eprintln!("Error: cannot create .gitim directory: {e}");
        process::exit(1);
    });

    if args.debug_http {
        ensure_config_debug_http(&repo_dir, true);
    }

    if let Err(e) = ensure_daemon(&repo_dir) {
        eprintln!("Error: failed to start daemon: {e}");
        process::exit(1);
    }

    let client = GitimClient::new(&repo_dir);
    let auth = if args.guest {
        None
    } else {
        Some(build_auth(&git_server, &args))
    };

    match client
        .onboard(git_server.as_str(), auth, args.admin, args.guest, true)
        .await
    {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Onboard 失败：{msg}");
                process::exit(1);
            }
            if args.guest {
                println!("游客模式已启动 @ {repo_name}");
            } else {
                let handler = resp
                    .data
                    .as_ref()
                    .and_then(|d| d.get("handler"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("(unknown)");
                let created = resp
                    .data
                    .as_ref()
                    .and_then(|d| d.get("created"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let created_label = if created {
                    "（新建）"
                } else {
                    "（已加入）"
                };
                let admin_tag = if args.admin { " [ADMIN]" } else { "" };
                println!("成功 {created_label}：@{handler}{admin_tag} @ {repo_name}");
            }
        }
        Err(e) => {
            eprintln!("Onboard 失败：{e}");
            process::exit(1);
        }
    }
}
