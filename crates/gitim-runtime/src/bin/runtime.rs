use std::net::SocketAddr;
use std::path::PathBuf;

use gitim_runtime::{provision_agent, AgentConfig, AgentLoop};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let args: Vec<String> = std::env::args().collect();

    // Shell mode: gitim-runtime --port <PORT>
    if args.len() >= 3 && args[1] == "--port" {
        let port: u16 = args[2].parse().expect("invalid port number");
        return run_shell(port).await;
    }

    // Legacy agent mode: gitim-runtime <remote_url> <handler> <display_name> [agents_dir]
    if args.len() < 4 {
        eprintln!("Usage:");
        eprintln!("  gitim-runtime --port <PORT>                              (shell mode)");
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

async fn run_shell(port: u16) -> Result<(), Box<dyn std::error::Error>> {
    let (router, state) = gitim_runtime::http::create_router();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

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
        repos.push(agent.repo_root.clone());
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
