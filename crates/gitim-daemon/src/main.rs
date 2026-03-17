mod api;
mod error;
mod handlers;
mod http;
mod lifecycle;
mod server;
mod state;

use std::sync::Arc;
use tracing::info;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let repo_root = std::env::current_dir()?;
    let lifecycle = lifecycle::DaemonLifecycle::new(&repo_root);

    if let Some(pid) = lifecycle.is_running() {
        eprintln!("daemon already running (pid: {})", pid);
        std::process::exit(1);
    }

    // Load config
    let config_path = repo_root.join(".gitim").join("config.yaml");
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|e| format!("failed to read config: {}", e))?;
    let config = gitim_core::validator::validate_config(&config_str)
        .map_err(|e| format!("invalid config: {}", e))?;

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
    let me_path = repo_root.join(".gitim").join("me.json");
    let current_user: Option<String> = if me_path.exists() {
        let me_content = std::fs::read_to_string(&me_path)?;
        let me_json: serde_json::Value = serde_json::from_str(&me_content)?;
        me_json.get("handler").and_then(|v| v.as_str()).map(|s| s.to_string())
    } else {
        tracing::warn!("no .gitim/me.json found, running without identity");
        None
    };

    if let Some(ref user) = current_user {
        tracing::info!("daemon identity: @{}", user);
    }

    let debug_http = config.daemon.debug_http;
    let debug_port = config.daemon.debug_port;

    let app_state = Arc::new(state::AppState::new(repo_root.clone(), config, current_user));
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

    // Start sync loop
    let sync_interval = app_state.config.daemon.sync_interval;
    let sync_root = repo_root.clone();
    tokio::spawn(async move {
        gitim_sync::sync_loop::start_sync_loop(&sync_root, sync_interval).await;
    });

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
