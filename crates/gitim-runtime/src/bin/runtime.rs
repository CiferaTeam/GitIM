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
    let router = gitim_runtime::http::create_router();
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    eprintln!("runtime shell listening on http://{addr}");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router).await?;
    Ok(())
}
