#![allow(dead_code, clippy::print_stdout, clippy::print_stderr)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]

use std::{path::Path, sync::Arc};
use tokio::sync::broadcast;
use tracing::info;

use gitim_daemon::{api, http, lifecycle, server, state};

type DaemonIdentity = (Option<String>, bool, bool, Option<String>);

fn read_identity_from_me(repo_root: &Path) -> Result<DaemonIdentity, Box<dyn std::error::Error>> {
    let me_path = repo_root.join(".gitim").join("me.json");
    if !me_path.exists() {
        return Ok((None, false, false, None));
    }

    let me_content = std::fs::read_to_string(&me_path)?;
    let me: gitim_core::me_json::MeJson = serde_json::from_str(&me_content)?;
    let email = me.github_email.filter(|s| !s.is_empty());
    Ok((
        me.handler,
        me.guest.unwrap_or(false),
        me.admin.unwrap_or(false),
        email,
    ))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // --version must come before tracing init to keep output clean
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("gitim-daemon {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

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
            if name.ends_with(".meta.yaml") {
                let handler = name.trim_end_matches(".meta.yaml").to_string();
                users.push(handler);
            }
        }
    }

    // Read identity from .gitim/me.json (written by CLI onboard)
    // Absence is normal on first startup before onboard — not an error
    let (current_user, is_guest_from_me, is_admin_from_me, github_email) =
        read_identity_from_me(&repo_root)?;

    if let Some(ref user) = current_user {
        tracing::info!("daemon identity: @{}", user);
    }
    if github_email.is_some() {
        tracing::info!("daemon commit author email: configured (from me.json)");
    }

    let debug_http = config.daemon.debug_http;
    let debug_port = config.daemon.debug_port;

    let (event_tx, _) = broadcast::channel::<api::Event>(256);
    let app_state = Arc::new(state::AppState::new_with_email(
        repo_root.clone(),
        config,
        event_tx,
        current_user,
        github_email,
    ));
    {
        let mut u = app_state.users.write().await;
        *u = users;
    }

    if is_guest_from_me {
        app_state
            .is_guest
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("daemon identity: guest mode");
    }
    if is_admin_from_me {
        app_state
            .is_admin
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tracing::info!("daemon identity: admin mode (from me.json)");
    }

    lifecycle.ensure_run_dir()?;
    lifecycle.write_pid()?;

    let lc = lifecycle::DaemonLifecycle::new(&repo_root);
    tokio::spawn(async move {
        shutdown_signal().await;
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

    // Load gitim.epoch.yaml once at boot so the daemon knows whether this
    // repo is on an active or redirected epoch. Missing file is normal
    // (legacy repos, fresh clones predating snapshot pack); parse failures
    // are logged but do not abort startup — Subtask C's write gate would
    // simply treat the state as Active until the next sync cycle reads a
    // valid file.
    if let Err(e) = app_state.refresh_epoch_status() {
        tracing::warn!("epoch status refresh on boot failed: {}", e);
    }

    // Epoch rotation crash recovery (design scenario 7). Two residue shapes:
    //   - HEAD redirected, origin active  → a fire died before its atomic
    //     push; the local redirect commit was never published. Clean it up
    //     (subjects + dirty-tree verification inside refuse unsafe resets).
    //   - origin redirected               → a rotation completed (ours or
    //     another daemon's) while we were down. Follow it now.
    // Holds commit_lock — handlers racing boot serialize behind it.
    {
        let storage = gitim_sync::git::GitStorage::new(&app_state.repo_root);
        if storage.has_remote() {
            if let Ok(branch) = storage.current_branch() {
                let _ = storage.fetch();
                let origin_redirected = matches!(
                    gitim_sync::rotate::epoch_status_at_ref(&storage, &format!("origin/{branch}")),
                    Ok(Some(gitim_core::epoch::EpochStatus::Redirected))
                );
                let head_redirected = matches!(
                    gitim_sync::rotate::epoch_status_at_ref(&storage, "HEAD"),
                    Ok(Some(gitim_core::epoch::EpochStatus::Redirected))
                );
                if head_redirected && !origin_redirected {
                    let orphan = gitim_sync::rotate::epoch_file_at_ref(&storage, "HEAD")
                        .ok()
                        .flatten()
                        .and_then(|f| f.redirect.map(|r| r.target_branch))
                        .unwrap_or_default();
                    let _guard = app_state
                        .commit_lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Err(e) =
                        gitim_sync::rotate::cleanup_failed_fire(&storage, &branch, &orphan)
                    {
                        tracing::warn!("boot: partial-fire cleanup failed: {}", e);
                    } else {
                        let _ = app_state.refresh_epoch_status();
                    }
                } else if origin_redirected {
                    let _guard = app_state
                        .commit_lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    match gitim_sync::rotate::follow_redirect(&storage, &branch) {
                        Ok(true) => {
                            tracing::info!("boot: followed epoch redirect off {branch}");
                            let _ = app_state.refresh_epoch_status();
                        }
                        Ok(false) => {}
                        Err(e) => tracing::warn!("boot: follow_redirect failed: {}", e),
                    }
                }
            }
        }
    }

    // Reconcile any legacy orphan cards from pre-Task-3.x archive_channel
    // implementations that only moved channel meta+thread, leaving
    // channels/<ch>/cards/ behind. This is a one-shot boot-time migration;
    // when there is nothing to do (the common case) it exits immediately.
    //
    // NOTE: the HTTP server is already live at this point (spawned above).
    // reconcile_orphan_cards holds state.commit_lock for its full execution
    // to prevent concurrent incoming handlers from racing on git tree writes.
    if let Err(e) = gitim_daemon::reconcile::reconcile_orphan_cards(app_state.clone()).await {
        tracing::error!("reconcile_orphan_cards failed at boot: {}", e);
        // Non-fatal — proceed to handler loop; sync_loop will pick up on next cycle.
    }

    // Start sync loop only if identity is already configured (restart scenario).
    // On first startup (no me.json), the sync loop is deferred until after onboard.
    if app_state.current_user.read().await.is_some() || is_guest_from_me {
        state::AppState::spawn_sync_loop(app_state.clone());
        // Cron engine runs in parallel to sync_loop; same identity gate
        // because the engine's ownership filter compares spec.target to
        // the daemon's running handler. Guest mode skips the engine —
        // there's no resolved target to compare against.
        if !is_guest_from_me {
            state::AppState::spawn_cron_engine(app_state.clone());
        }
    } else {
        info!("no identity configured — sync loop deferred until onboard");
    }

    // Start file watcher
    let (watcher_tx, mut watcher_rx) = tokio::sync::mpsc::channel(100);
    gitim_sync::watcher::watch_repo(&repo_root, watcher_tx)
        .await
        .ok();

    // Process watcher events - invalidate cache
    let watcher_state = app_state.clone();
    tokio::spawn(async move {
        while let Some(event) = watcher_rx.recv().await {
            match event {
                gitim_sync::watcher::FileEvent::ThreadModified(name) => {
                    tracing::debug!("thread modified: {}", name);
                    watcher_state.thread_cache.write().await.remove(&name);
                    // Safe: handler/channel names must not contain "--"
                    // (per handler/channel naming rules), so "--" only
                    // appears in DM filenames as the separator.
                    let kind = if name.contains("--") { "dm" } else { "channel" };
                    let _ = watcher_state
                        .event_tx
                        .send(gitim_daemon::api::Event::ThreadChanged {
                            channel: name,
                            kind: kind.to_string(),
                        });
                }
                gitim_sync::watcher::FileEvent::MetaModified(name) => {
                    tracing::debug!("meta modified: {}", name);
                }
                gitim_sync::watcher::FileEvent::FlowModified(slug) => {
                    tracing::debug!("flow modified: {}", slug);
                    let flow_root = watcher_state.repo_root.join("flows").join(&slug);
                    let index_md = flow_root.join("index.md");
                    if !index_md.exists() {
                        continue;
                    }
                    {
                        let _guard = watcher_state
                            .commit_lock
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        match std::fs::read_to_string(&index_md) {
                            Ok(content) => {
                                let rel_path = format!("flows/{}/index.md", slug);
                                match gitim_core::flow::parse_flow_markdown(&content) {
                                    Ok(doc) => {
                                        if let Err(e) =
                                            gitim_core::flow::validate_flow_document(&doc, &slug)
                                        {
                                            tracing::warn!(
                                                "flow {} validation failed: {} — committing anyway",
                                                slug,
                                                e
                                            );
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!(
                                            "flow {} parse failed: {} — committing anyway",
                                            slug,
                                            e
                                        );
                                    }
                                }
                                let (name, email) = watcher_state.author_for("system");
                                let _ = watcher_state.git_storage.add_and_commit_only_as(
                                    &rel_path,
                                    &format!("flow: edit {} @system", slug),
                                    Some((&name, &email)),
                                );
                            }
                            Err(e) => {
                                tracing::warn!("flow {} read failed: {}", slug, e);
                            }
                        }
                    } // _guard drops here
                    let _ = watcher_state
                        .event_tx
                        .send(gitim_daemon::api::Event::FlowChanged { slug });
                    watcher_state.push_notify.notify_one();
                }
            }
        }
    });

    // Idle watchdog: exit silently if no client connects for 24 hours
    let idle_lc = lifecycle::DaemonLifecycle::new(&repo_root);
    let idle_state = app_state.clone();
    tokio::spawn(async move {
        const IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
        const CHECK_INTERVAL_SECS: u64 = 60 * 60;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
            let last = idle_state
                .last_client_activity
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if now.saturating_sub(last) >= IDLE_TIMEOUT_SECS {
                info!("no client activity for 24h — shutting down");
                idle_lc.cleanup();
                std::process::exit(0);
            }
        }
    });

    let socket_path = lifecycle.socket_path();
    server::start_unix_socket(&socket_path, app_state).await?;

    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        if let Ok(mut sigterm) = signal(SignalKind::terminate()) {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {},
                _ = sigterm.recv() => {},
            }
        } else {
            let _ = tokio::signal::ctrl_c().await;
        }
    }

    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_identity_from_me_restores_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let gitim_dir = tmp.path().join(".gitim");
        std::fs::create_dir_all(&gitim_dir).unwrap();
        std::fs::write(
            gitim_dir.join("me.json"),
            r#"{
                "handler": "alice",
                "admin": true,
                "guest": false,
                "github_email": "alice@example.com"
            }"#,
        )
        .unwrap();

        let (handler, guest, admin, email) = read_identity_from_me(tmp.path()).unwrap();
        assert_eq!(handler.as_deref(), Some("alice"));
        assert!(!guest);
        assert!(admin);
        assert_eq!(email.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn read_identity_from_me_missing_file_defaults_to_non_admin() {
        let tmp = tempfile::tempdir().unwrap();
        let (handler, guest, admin, email) = read_identity_from_me(tmp.path()).unwrap();
        assert_eq!(handler, None);
        assert!(!guest);
        assert!(!admin);
        assert_eq!(email, None);
    }
}
