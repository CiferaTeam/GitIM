use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::http::DEFAULT_PORT;
use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};

fn cleanup_pid_file() {
    if let Some(home) = dirs::home_dir() {
        let _ = std::fs::remove_file(home.join(".gitim/runtime.pid"));
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // --version: print and exit before anything else
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("gitim-runtime {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    gitim_runtime::tool_path::ensure_common_tool_paths();

    tracing_subscriber::fmt::init();

    // Environment preflight: all three binaries must be version-aligned
    if let Err(e) = gitim_runtime::preflight::check_env() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    // Parse flags
    let daemon = args.iter().any(|a| a == "--daemon" || a == "-d");
    let port = parse_port(&args);

    // Shell mode: no positional args, or --port/--daemon present
    if daemon || port.is_some() || args.len() == 1 {
        let port = port.unwrap_or(DEFAULT_PORT);
        if daemon {
            return daemonize(port);
        }
        return run_shell(port).await;
    }

    // Legacy agent mode: gitim-runtime <remote_url> <handler> <display_name> [agents_dir]
    if args.len() < 4 {
        eprintln!("Usage:");
        eprintln!("  gitim-runtime [--port <PORT>] [-d|--daemon]               (shell mode, default port {DEFAULT_PORT})");
        eprintln!(
            "  gitim-runtime <remote_url> <handler> <display_name> [agents_dir]  (agent mode)"
        );
        std::process::exit(1);
    }

    let remote_url = &args[1];
    let handler = &args[2];
    let display_name = &args[3];
    let agents_dir = if args.len() > 4 {
        PathBuf::from(&args[4])
    } else {
        dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("gitim-agents")
    };

    std::fs::create_dir_all(&agents_dir)?;

    eprintln!("provisioning agent @{handler} ...");
    let config = AgentConfig {
        handler: handler.clone(),
        display_name: display_name.clone(),
        remote_url: remote_url.clone(),
        github_email: None,
    };
    let handle = provision_agent(&agents_dir, &config, true).await?;
    eprintln!("agent ready at {}", handle.repo_root.display());

    eprintln!("starting agent loop (ctrl-c to stop) ...");
    let mut agent_loop = AgentLoop::with_defaults(&handle.repo_root)?;
    agent_loop.run().await?;

    Ok(())
}

fn parse_port(args: &[String]) -> Option<u16> {
    args.windows(2)
        .find(|w| w[0] == "--port")
        .map(|w| w[1].parse().expect("invalid port number"))
}

fn daemonize(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let exe = std::env::current_exe()?;

    // Runtime + per-daemon logs both live in ~/.gitim/logs/ so a single
    // tail over the directory surfaces all agent activity.
    let log_path = gitim_runtime::daemon_log::runtime_log_path();
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let log_file = std::fs::File::create(&log_path)?;

    // PID file ownership lives with the process actually serving HTTP —
    // `run_shell()` writes it at startup. That way a future self-replace
    // path (fork-exec a fresh runtime with new binary) doesn't need to
    // also remember to rewrite the PID file from the exiting parent.
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file);
    let child = gitim_runtime::background::spawn_detached(&mut cmd)?;

    eprintln!(
        "runtime started in background (pid: {}, port: {port})",
        child.id()
    );
    eprintln!("log: {}", log_path.display());

    Ok(())
}

async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    // Capture canonical exe BEFORE any self-replace could run. After
    // replace_binaries swaps the on-disk file, Linux `current_exe()` returns
    // "<path> (deleted)" for this inode — too late then. Stored in
    // RuntimeState so the self-update endpoint can strict-mode-check the
    // install dir and pick the fork-exec target.
    let canonical_exe = std::env::current_exe()?.canonicalize()?;

    // Whoever is actually serving HTTP owns the PID file. On normal boot
    // this is just us writing our own pid; on self-replace restart the
    // freshly spawned runtime overwrites whatever the dying parent left.
    let pid_path = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gitim/runtime.pid");
    if let Some(parent) = pid_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&pid_path, std::process::id().to_string())?;

    let (router, state) = gitim_runtime::http::create_router_with_exe(canonical_exe);
    // Record the port we're about to bind so the self-update async phase can
    // pass the same `--port` to the replacement runtime. `run_shell` is the
    // single writer; nothing else in the crate needs to mutate this.
    state.lock().unwrap().listen_port = port;

    // Materialize the device-bound runtime ID. First boot generates and
    // persists; subsequent boots read the existing UUID. Either way it lands
    // in RuntimeState before recover_from_config so /health responds with the
    // real ID even during the recovery window.
    // See docs/plans/runtime-id/00-design.md.
    let runtime_id = gitim_runtime::user_config::ensure_runtime_id();
    state.lock().unwrap().runtime_id = runtime_id.clone();
    eprintln!("runtime started, id: {runtime_id}");

    // Token + email propagation MUST run before `recover_from_config`, because
    // recovery spawns per-agent daemons and each daemon reads `me.json` /
    // `.git/config` into memory at startup. If we propagate after, the daemons
    // are already running with stale values and the fix won't take effect until
    // the user manually restarts them — which nobody knows to do.
    //
    // Both propagation passes are file-only (no state dependency), so we can
    // drive them straight from `user_config::read()` instead of from the
    // runtime state populated by recovery.
    let pre_recovery_paths: Vec<PathBuf> = gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .map(|w| PathBuf::from(&w.path))
        .filter(|p| p.exists())
        .collect();

    // If config.json's token was edited while the runtime was down, clones
    // still carry the old token. Resync on startup so fetch/push don't fail.
    for workspace in &pre_recovery_paths {
        if let Err(e) = gitim_runtime::token_propagation::propagate_token(workspace) {
            tracing::warn!(error = %e, "token propagation on startup failed");
        }
    }

    // Backfill `github_email` for workspaces that predate the email feature
    // (or were provisioned when /user.email came back null). Net effect is
    // that existing github-mode workspaces start crediting commits to the
    // owner's contribution graph on the next runtime boot, no re-init and
    // no manual daemon restart needed.
    for workspace in &pre_recovery_paths {
        match gitim_runtime::email_propagation::backfill_github_email(
            workspace,
            gitim_runtime::email_propagation::GITHUB_API_BASE,
        )
        .await
        {
            Ok(true) => {
                tracing::info!(
                    workspace = %workspace.display(),
                    "email backfill applied",
                );
            }
            Ok(false) => {}
            Err(e) => {
                tracing::warn!(error = %e, "email backfill on startup failed");
            }
        }
    }

    gitim_runtime::http::recover_from_config(state.clone()).await;

    // Idle watchdog: exit if no activity for 24 hours
    let idle_state = state.clone();
    tokio::spawn(async move {
        const IDLE_TIMEOUT_SECS: u64 = 24 * 60 * 60;
        const CHECK_INTERVAL_SECS: u64 = 60 * 60;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)).await;
            let last = idle_state
                .lock()
                .unwrap()
                .last_activity
                .load(std::sync::atomic::Ordering::Relaxed);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            if now.saturating_sub(last) >= IDLE_TIMEOUT_SECS {
                if gitim_runtime::http::has_active_agents(&idle_state) {
                    eprintln!("idle timeout reached but agents still active, deferring exit");
                    continue;
                }
                eprintln!("no activity for 24h — shutting down");
                cleanup_pid_file();
                gitim_runtime::workspace::kill_managed_daemons(&idle_state);
                std::process::exit(0);
            }
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    // Self-update path fork-execs a fresh runtime and then `exit(0)`s the
    // parent. The child can briefly race the parent for the listening port:
    // parent hasn't released it yet when child first calls bind. Retry a few
    // times on AddrInUse so the child survives that ~100ms window instead of
    // dying and leaving the frontend polling a dead `/health`.
    // 10 x 100ms = 1s max wait, well over the observed race window.
    let listener = {
        let mut attempts = 0;
        loop {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => break l,
                Err(e) if attempts < 10 && e.kind() == std::io::ErrorKind::AddrInUse => {
                    attempts += 1;
                    tracing::warn!(
                        ?e,
                        attempts,
                        "port in use (likely restart race), retrying in 100ms"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                }
                Err(e) => return Err(e.into()),
            }
        }
    };
    let mut server = tokio::spawn(async move { axum::serve(listener, router).await });

    // Wait for shutdown signal; also bail if the server itself errors out
    tokio::select! {
        _ = shutdown_signal() => {},
        result = &mut server => {
            if let Err(e) = result? {
                eprintln!("server error: {e}");
            }
        }
    }

    // SSE keep-alive connections block axum graceful shutdown indefinitely;
    // abort the server task so the process can exit cleanly.
    server.abort();

    // Kill all managed daemons on shutdown
    cleanup_pid_file();
    gitim_runtime::workspace::kill_managed_daemons(&state);
    eprintln!("all daemons stopped");
    Ok(())
}

async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let mut sigterm = signal(SignalKind::terminate()).expect("failed to install SIGTERM handler");

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {},
        _ = sigterm.recv() => {},
    }

    eprintln!("\nshutting down...");
}
