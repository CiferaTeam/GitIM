mod api;
mod error;
mod http;
mod lifecycle;
mod server;

use std::path::PathBuf;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let repo_root = PathBuf::from(".");
    let lifecycle = lifecycle::DaemonLifecycle::new(&repo_root);

    if let Some(pid) = lifecycle.is_running() {
        eprintln!("daemon already running (pid: {})", pid);
        std::process::exit(1);
    }

    lifecycle.ensure_run_dir()?;
    lifecycle.write_pid()?;

    let lc = lifecycle::DaemonLifecycle::new(&repo_root);
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        lc.cleanup();
        std::process::exit(0);
    });

    // TODO: Start HTTP debug server when config.daemon.debug_http is true
    // Will be wired in Task I.1 when config loading is implemented

    let socket_path = lifecycle.socket_path();
    server::start_unix_socket(&socket_path).await?;

    Ok(())
}
