use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

use gitim_runtime::http::DEFAULT_PORT;

fn runtime_pid_path() -> Option<PathBuf> {
    dirs::home_dir().map(|home| home.join(".gitim/runtime.pid"))
}

fn runtime_pid_file_points_to_current_process() -> bool {
    let Some(pid_path) = runtime_pid_path() else {
        return true;
    };
    pid_file_points_to_process(&pid_path, std::process::id())
}

fn pid_file_points_to_process(pid_path: &Path, pid: u32) -> bool {
    match std::fs::read_to_string(pid_path) {
        Ok(recorded) => recorded.trim() == pid.to_string(),
        Err(_) => true,
    }
}

fn cleanup_pid_file() {
    let Some(pid_path) = runtime_pid_path() else {
        return;
    };
    if runtime_pid_file_points_to_current_process() {
        let _ = std::fs::remove_file(pid_path);
    }
}

/// gitim-runtime: dual-mode binary.
///
/// No subcommand: runs the HTTP server (default; backs the WebUI and agent
/// lifecycle). With a subcommand: one-shot CLI that shells out to a running
/// runtime over HTTP, so AI agents and scripts can drive the runtime without
/// the WebUI.
///
/// Subcommand bodies are placeholders in this scaffolding pass — actual
/// behavior lands in later tasks (Tasks 6-12 of the runtime-cli plan).
#[derive(Parser, Debug)]
#[command(name = "gitim-runtime", version, about, long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,

    /// Port to bind the HTTP server on (server mode only).
    #[arg(long)]
    port: Option<u16>,

    /// Daemonize: fork-exec a detached server and exit (server mode only).
    #[arg(long, short = 'd')]
    daemon: bool,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Show runtime status (running/stopped, port, version).
    Status,
    /// Print the device-bound runtime ID.
    RuntimeId,
    /// List workspaces known to the runtime.
    Workspaces,
    /// List agents in a workspace.
    ListAgents,
    /// Provision a new agent in a workspace.
    AddAgent,
    /// Hard-delete an agent (irreversible).
    BurnAgent,
    /// Update an existing agent's editable fields.
    UpdateAgent,
    /// Run provider preflight checks (binary present, version, hello round-trip).
    Preflight,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    match args.command {
        None => run_server(args.port, args.daemon).await,
        Some(cmd) => run_cli(cmd),
    }
}

/// One-shot CLI dispatch. Each variant's body is filled in by later tasks
/// in the runtime-cli plan (Tasks 6-12); this scaffold just establishes the
/// command surface so the clap derive compiles and `--help` is meaningful.
///
/// Tracing is initialized at WARN level (not INFO like server mode) so the
/// CLI's JSON stdout output stays clean for downstream parsing.
fn run_cli(cmd: Command) -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .init();

    match cmd {
        Command::Status => todo!("subcommand `status` — implemented in later task"),
        Command::RuntimeId => todo!("subcommand `runtime-id` — implemented in later task"),
        Command::Workspaces => todo!("subcommand `workspaces` — implemented in later task"),
        Command::ListAgents => todo!("subcommand `list-agents` — implemented in later task"),
        Command::AddAgent => todo!("subcommand `add-agent` — implemented in later task"),
        Command::BurnAgent => todo!("subcommand `burn-agent` — implemented in later task"),
        Command::UpdateAgent => todo!("subcommand `update-agent` — implemented in later task"),
        Command::Preflight => todo!("subcommand `preflight` — implemented in later task"),
    }
}

/// Server mode: same boot path as before the CLI split. Initializes tracing,
/// runs env preflight, then either daemonizes or runs the shell directly.
async fn run_server(port: Option<u16>, daemon: bool) -> Result<(), Box<dyn std::error::Error>> {
    gitim_runtime::tool_path::ensure_common_tool_paths();

    tracing_subscriber::fmt::init();

    // Environment preflight: all three binaries must be version-aligned
    if let Err(e) = gitim_runtime::preflight::check_env() {
        eprintln!("{e}");
        std::process::exit(1);
    }

    let port = port.unwrap_or(DEFAULT_PORT);
    if daemon {
        return daemonize(port);
    }
    run_shell(port).await
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
                if runtime_pid_file_points_to_current_process() {
                    cleanup_pid_file();
                    gitim_runtime::workspace::kill_managed_daemons(&idle_state);
                }
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

    // Persist the actually-bound port as a CLI discovery hint. Best-effort:
    // a failure here doesn't block serving — the CLI falls back to
    // DEFAULT_PORT if this hint is missing or stale.
    if let Err(e) = gitim_runtime::user_config::write_listen_port(port) {
        tracing::warn!(error = %e, port, "failed to persist listen_port hint");
    }

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
    if runtime_pid_file_points_to_current_process() {
        cleanup_pid_file();
        gitim_runtime::workspace::kill_managed_daemons(&state);
        eprintln!("all daemons stopped");
    } else {
        eprintln!(
            "runtime pid changed; assuming replacement runtime took over, skipping daemon stop"
        );
    }
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

#[cfg(test)]
mod pid_file_tests {
    use super::*;

    #[test]
    fn pid_file_owner_matches_expected_process() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("runtime.pid");
        std::fs::write(&path, "12345\n").expect("pid file");

        assert!(pid_file_points_to_process(&path, 12345));
        assert!(!pid_file_points_to_process(&path, 54321));
    }

    #[test]
    fn missing_pid_file_keeps_current_process_responsible() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("missing.pid");

        assert!(pid_file_points_to_process(&path, 12345));
    }
}

#[cfg(test)]
mod argv_dispatch_tests {
    //! Argv parsing boundary tests. These verify the basic dispatch contract:
    //! no-subcommand → server mode, subcommand → CLI mode, and server-only
    //! flags (`--port`, `--daemon`) are rejected when a subcommand is present
    //! so they can't be silently ignored. The full per-subcommand argv test
    //! catalog lives in Task 13.
    use super::*;
    use clap::Parser;

    #[test]
    fn no_args_means_server_mode() {
        let args = Args::try_parse_from(["gitim-runtime"]).expect("parse must succeed");
        assert!(args.command.is_none());
        assert!(!args.daemon);
        assert!(args.port.is_none());
    }

    #[test]
    fn port_flag_at_top_level() {
        let args = Args::try_parse_from(["gitim-runtime", "--port", "5000"])
            .expect("parse must succeed");
        assert!(args.command.is_none());
        assert_eq!(args.port, Some(5000));
    }

    #[test]
    fn daemon_flag_at_top_level() {
        let args = Args::try_parse_from(["gitim-runtime", "-d"]).expect("parse must succeed");
        assert!(args.command.is_none());
        assert!(args.daemon);
    }

    #[test]
    fn subcommand_alone() {
        let args = Args::try_parse_from(["gitim-runtime", "status"]).expect("parse must succeed");
        assert!(matches!(args.command, Some(Command::Status)));
    }

    #[test]
    fn port_with_subcommand_rejected() {
        // --port is server-mode-only; combining it with a subcommand should
        // be an "unexpected argument" error, not a silent no-op.
        let result = Args::try_parse_from(["gitim-runtime", "status", "--port", "8080"]);
        assert!(result.is_err());
    }

    #[test]
    fn legacy_positional_rejected() {
        // The pre-CLI positional form (`gitim-runtime <url> <handler> <name>`)
        // must not parse as a subcommand or as bare server-mode args.
        let result = Args::try_parse_from([
            "gitim-runtime",
            "https://github.com/o/r",
            "handler",
            "name",
        ]);
        assert!(result.is_err());
    }

    #[test]
    fn unknown_subcommand_rejected() {
        let result = Args::try_parse_from(["gitim-runtime", "fly-to-mars"]);
        assert!(result.is_err());
    }
}
