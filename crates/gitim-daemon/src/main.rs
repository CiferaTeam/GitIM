#![deny(warnings)]
#![allow(dead_code)]

use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::info;

use gitim_daemon::{api, http, lifecycle, server, state};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let repo_root = std::env::current_dir()?;
    let lifecycle = lifecycle::DaemonLifecycle::new(&repo_root);

    if let Some(pid) = lifecycle.is_running() {
        eprintln!("daemon already running (pid: {})", pid);
        std::process::exit(1);
    }

    // Load config, creating default if missing
    let gitim_dir = repo_root.join(".gitim");
    let config_path = gitim_dir.join("config.yaml");
    let config = if config_path.exists() {
        let config_str = std::fs::read_to_string(&config_path)
            .map_err(|e| format!("failed to read config: {}", e))?;
        gitim_core::validator::validate_config(&config_str)
            .map_err(|e| format!("invalid config: {}", e))?
    } else {
        let default_config = gitim_core::types::config::Config::default();
        let yaml = serde_yaml::to_string(&default_config)
            .map_err(|e| format!("failed to serialize default config: {}", e))?;
        std::fs::create_dir_all(&gitim_dir)
            .map_err(|e| format!("failed to create .gitim dir: {}", e))?;
        std::fs::write(&config_path, &yaml)
            .map_err(|e| format!("failed to write default config: {}", e))?;
        info!("created default config at {}", config_path.display());
        default_config
    };

    // Scan users
    let users_dir = repo_root.join("users");
    let mut users = Vec::new();
    if users_dir.exists() {
        for entry in std::fs::read_dir(&users_dir)? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".meta.json") {
                let handler = name.trim_end_matches(".meta.json").to_string();
                users.push(handler);
            }
        }
    }

    // Read identity from .gitim/me.json (written by CLI onboard)
    // Absence is normal on first startup before onboard — not an error
    let me_path = repo_root.join(".gitim").join("me.json");
    let current_user: Option<String> = if me_path.exists() {
        let me_content = std::fs::read_to_string(&me_path)?;
        let me_json: serde_json::Value = serde_json::from_str(&me_content)?;
        me_json.get("handler").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        None
    };

    if let Some(ref user) = current_user {
        tracing::info!("daemon identity: @{}", user);
    }

    let debug_http = config.daemon.debug_http;
    let debug_port = config.daemon.debug_port;

    let (event_tx, _) = broadcast::channel::<api::Event>(256);
    let app_state = Arc::new(state::AppState::new(repo_root.clone(), config, event_tx, current_user));
    {
        let mut u = app_state.users.write().await;
        *u = users;
    }

    lifecycle.ensure_run_dir()?;
    lifecycle.write_pid()?;

    let lc = lifecycle::DaemonLifecycle::new(&repo_root);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        lc.cleanup();
        std::process::exit(0);
    });

    // Start HTTP debug server if enabled
    if debug_http {
        let router = http::create_router(app_state.clone());
        let addr = format!("0.0.0.0:{}", debug_port);
        info!("HTTP debug server on {}", addr);
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        lifecycle::DaemonLifecycle::new(&repo_root).write_port(debug_port)?;
        tokio::spawn(async move {
            axum::serve(listener, router).await.ok();
        });
    }

    // Initialize search index (best effort — search is unavailable if this fails)
    if let Err(e) = state::AppState::initialize_index(&app_state) {
        tracing::warn!("index initialization failed (search unavailable): {}", e);
    }

    // Start sync loop only if identity is already configured (restart scenario).
    // On first startup (no me.json), the sync loop is deferred until after onboard.
    if app_state.current_user.read().await.is_some() {
        state::AppState::spawn_sync_loop(app_state.clone());
    } else {
        info!("no identity configured — sync loop deferred until onboard");
    }

    // Start file watcher
    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(100);
    gitim_sync::watcher::watch_repo(&repo_root, watcher_tx).await.ok();

    // Process watcher events - invalidate cache
    let watcher_state = app_state.clone();
    tokio::spawn(async move {
        while let Some(event) = watcher_rx.recv().await {
            match event {
                gitim_sync::watcher::FileEvent::ThreadModified(name) => {
                    tracing::debug!("thread modified: {}", name);
                    watcher_state.thread_cache.write().await.remove(&name);
                    // Safe: handler/channel names MUST NOT contain "--" (spec §3.2, §4.1)
                    // so "--" only appears in DM filenames as the separator
                    let kind = if name.contains("--") { "dm" } else { "channel" };
                    let _ = watcher_state.event_tx.send(gitim_daemon::api::Event::ThreadChanged {
                        channel: name,
                        kind: kind.to_string(),
                    });
                }
                gitim_sync::watcher::FileEvent::MetaModified(name) => {
                    tracing::debug!("meta modified: {}", name);
                    // Could trigger user list refresh here
                }
            }
        }
    });

    let socket_path = lifecycle.socket_path();
    server::start_unix_socket(&socket_path, app_state).await?;

    Ok(())
}
