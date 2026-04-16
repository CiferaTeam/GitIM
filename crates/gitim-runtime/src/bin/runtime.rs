use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};

const DEFAULT_PORT: u16 = 16868;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();

    // --version: print and exit before anything else
    if args.get(1).map(|s| s.as_str()) == Some("--version") {
        println!("gitim-runtime {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

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
        eprintln!("  gitim-runtime <remote_url> <handler> <display_name> [agents_dir]  (agent mode)");
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
    };
    let handle = provision_agent(&agents_dir, &config).await?;
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
    let gitim_dir = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".gitim");
    std::fs::create_dir_all(&gitim_dir)?;

    let log_path = gitim_dir.join("runtime.log");
    let pid_path = gitim_dir.join("runtime.pid");
    let log_file = std::fs::File::create(&log_path)?;

    let child = std::process::Command::new(exe)
        .args(["--port", &port.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(log_file.try_clone()?)
        .stderr(log_file)
        .spawn()?;

    std::fs::write(&pid_path, child.id().to_string())?;
    eprintln!("runtime started in background (pid: {}, port: {port})", child.id());
    eprintln!("log: {}", log_path.display());

    Ok(())
}

async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let (router, state) = gitim_runtime::http::create_router();

    // Recover previous workspace from ~/.gitim/runtime.json
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
                // Clean up pid file
                if let Some(home) = dirs::home_dir() {
                    let _ = std::fs::remove_file(home.join(".gitim/runtime.pid"));
                }
                kill_managed_daemons(&idle_state);
                std::process::exit(0);
            }
        }
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    let mut server = tokio::spawn(async move {
        axum::serve(listener, router).await
    });

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
    kill_managed_daemons(&state);
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

fn kill_managed_daemons(state: &gitim_runtime::http::SharedRuntimeState) {
    let s = state.lock().unwrap();

    // Collect all repo roots that have daemons
    let mut repos: Vec<PathBuf> = Vec::new();
    if let Some(ref human) = s.human_repo {
        repos.push(human.clone());
    }
    for agent in s.agents.values() {
        repos.push(PathBuf::from(&agent.repo_path));
    }
    drop(s);

    for repo in &repos {
        let pid_file = repo.join(".gitim/run/gitim.pid");
        if let Ok(content) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = content.trim().parse::<u32>() {
                // Use `kill` command to send SIGTERM
                let _ = std::process::Command::new("kill")
                    .arg(pid.to_string())
                    .output();
                eprintln!("killed daemon pid {pid} at {}", repo.display());
            }
        }
    }
}
