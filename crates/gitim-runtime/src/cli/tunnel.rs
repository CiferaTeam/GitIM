//! SSH tunnel lifecycle helpers for the runtime CLI.

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

use crate::cli::http::{CliError, Client};
use crate::user_config::{self, FleetNodeEntry};

const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);
const HEALTH_WAIT: Duration = Duration::from_secs(8);
const HEALTH_POLL: Duration = Duration::from_millis(200);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone)]
pub struct LaunchConfig {
    pub node_id: String,
    pub ssh_target: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelState {
    pub node_id: String,
    pub pid: u32,
    pub ssh_target: String,
    pub remote_host: String,
    pub remote_port: u16,
    pub local_port: u16,
    pub base_url: String,
    pub started_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelStatusKind {
    Up,
    Down,
    Stale,
}

#[derive(Debug, Clone)]
pub struct TunnelStatus {
    pub state: Option<TunnelState>,
    pub tunnel_status: TunnelStatusKind,
    pub runtime_healthy: bool,
    pub runtime_error: Option<String>,
}

pub fn base_url_for_port(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

pub fn resolve_local_port(local_port: Option<u16>) -> Result<u16, CliError> {
    match local_port {
        Some(0) => Err(CliError::InvalidConfig(
            "--local-port must be non-zero".to_string(),
        )),
        Some(port) => Ok(port),
        None => {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")
                .map_err(|e| CliError::Transport(format!("choose local port: {e}")))?;
            let port = listener
                .local_addr()
                .map_err(|e| CliError::Transport(format!("read local port: {e}")))?
                .port();
            Ok(port)
        }
    }
}

pub async fn client_for_node(node_id: &str) -> Result<Client, CliError> {
    let entry = find_node(node_id)?;
    if let Some(tunnel) = &entry.ssh_tunnel {
        let local_port = tunnel.local_port.ok_or_else(|| {
            CliError::InvalidConfig(format!(
                "fleet node {node_id} has ssh_tunnel but no local_port; run fleet tunnel up"
            ))
        })?;
        let launch = LaunchConfig {
            node_id: entry.node_id.clone(),
            ssh_target: tunnel.ssh_target.clone(),
            remote_host: tunnel.remote_host.clone(),
            remote_port: tunnel.remote_port,
            local_port,
        };
        let state = ensure_running(&launch).await?;
        return Ok(Client::new(state.base_url));
    }
    Ok(Client::new(entry.base_url))
}

pub fn find_node(node_id: &str) -> Result<FleetNodeEntry, CliError> {
    let needle = node_id.trim();
    if needle.is_empty() {
        return Err(CliError::InvalidConfig("--node is required".to_string()));
    }
    user_config::read()
        .fleet_nodes
        .into_iter()
        .find(|entry| entry.node_id == needle)
        .ok_or_else(|| CliError::InvalidConfig(format!("fleet node not found: {needle}")))
}

pub async fn ensure_running(config: &LaunchConfig) -> Result<TunnelState, CliError> {
    if let Some(state) = read_state(&config.node_id)? {
        if state.local_port == config.local_port && process_alive(state.pid) {
            match check_runtime_health(&state.base_url).await {
                Ok(()) => return Ok(state),
                Err(_) => {
                    stop_pid(state.pid).await;
                    remove_state(&config.node_id)?;
                }
            }
        } else {
            if process_alive(state.pid) {
                stop_pid(state.pid).await;
            }
            remove_state(&config.node_id)?;
        }
    }
    start_tunnel(config).await
}

pub async fn start_tunnel(config: &LaunchConfig) -> Result<TunnelState, CliError> {
    validate_launch_config(config)?;
    ensure_port_available(config.local_port)?;

    let log_path = tunnel_log_path(&config.node_id);
    if let Some(parent) = log_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::Transport(format!("create tunnel log dir: {e}")))?;
    }
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .map_err(|e| CliError::Transport(format!("open tunnel log: {e}")))?;

    let forward = format!(
        "127.0.0.1:{}:{}:{}",
        config.local_port, config.remote_host, config.remote_port
    );
    let mut cmd = Command::new("ssh");
    cmd.arg("-N")
        .arg("-L")
        .arg(forward)
        .args(["-o", "ExitOnForwardFailure=yes"])
        .args(["-o", "BatchMode=yes"])
        .args(["-o", "ServerAliveInterval=15"])
        .args(["-o", "ServerAliveCountMax=2"])
        .arg(&config.ssh_target)
        .stdin(Stdio::null())
        .stdout(
            log.try_clone()
                .map_err(|e| CliError::Transport(format!("clone tunnel log handle: {e}")))?,
        )
        .stderr(log);

    let child = crate::background::spawn_detached(&mut cmd)
        .map_err(|e| CliError::Transport(format!("spawn ssh tunnel: {e}")))?;
    let state = TunnelState {
        node_id: config.node_id.clone(),
        pid: child.id(),
        ssh_target: config.ssh_target.clone(),
        remote_host: config.remote_host.clone(),
        remote_port: config.remote_port,
        local_port: config.local_port,
        base_url: base_url_for_port(config.local_port),
        started_at: chrono::Utc::now().to_rfc3339(),
    };
    write_state(&state)?;

    if let Err(err) = wait_for_runtime_health(&state.base_url).await {
        stop_pid(state.pid).await;
        remove_state(&state.node_id)?;
        return Err(err);
    }

    Ok(state)
}

pub async fn status(node_id: &str) -> Result<TunnelStatus, CliError> {
    let Some(state) = read_state(node_id)? else {
        return Ok(TunnelStatus {
            state: None,
            tunnel_status: TunnelStatusKind::Down,
            runtime_healthy: false,
            runtime_error: None,
        });
    };
    if !process_alive(state.pid) {
        return Ok(TunnelStatus {
            state: Some(state),
            tunnel_status: TunnelStatusKind::Stale,
            runtime_healthy: false,
            runtime_error: Some("tunnel pid is not running".to_string()),
        });
    }
    match check_runtime_health(&state.base_url).await {
        Ok(()) => Ok(TunnelStatus {
            state: Some(state),
            tunnel_status: TunnelStatusKind::Up,
            runtime_healthy: true,
            runtime_error: None,
        }),
        Err(err) => Ok(TunnelStatus {
            state: Some(state),
            tunnel_status: TunnelStatusKind::Up,
            runtime_healthy: false,
            runtime_error: Some(err),
        }),
    }
}

pub async fn stop(node_id: &str) -> Result<Option<TunnelState>, CliError> {
    let state = read_state(node_id)?;
    if let Some(state) = &state {
        if process_alive(state.pid) {
            stop_pid(state.pid).await;
        }
    }
    remove_state(node_id)?;
    Ok(state)
}

fn validate_launch_config(config: &LaunchConfig) -> Result<(), CliError> {
    if config.node_id.trim().is_empty() {
        return Err(CliError::InvalidConfig("--node-id is required".to_string()));
    }
    if config.ssh_target.trim().is_empty() {
        return Err(CliError::InvalidConfig(
            "--ssh-target is required".to_string(),
        ));
    }
    if config.remote_host.trim().is_empty() {
        return Err(CliError::InvalidConfig(
            "--remote-host is required".to_string(),
        ));
    }
    if config.remote_port == 0 {
        return Err(CliError::InvalidConfig(
            "--remote-port must be non-zero".to_string(),
        ));
    }
    if config.local_port == 0 {
        return Err(CliError::InvalidConfig(
            "--local-port must be non-zero".to_string(),
        ));
    }
    Ok(())
}

fn ensure_port_available(port: u16) -> Result<(), CliError> {
    match std::net::TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            Ok(())
        }
        Err(e) => Err(CliError::Transport(format!(
            "local port {port} is unavailable: {e}"
        ))),
    }
}

async fn wait_for_runtime_health(base_url: &str) -> Result<(), CliError> {
    let deadline = tokio::time::Instant::now() + HEALTH_WAIT;
    loop {
        let err = match check_runtime_health(base_url).await {
            Ok(()) => return Ok(()),
            Err(err) => err,
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(CliError::Transport(format!(
                "remote runtime health check failed via tunnel: {err}"
            )));
        }
        tokio::time::sleep(HEALTH_POLL).await;
    }
}

async fn check_runtime_health(base_url: &str) -> Result<(), String> {
    let url = format!("{}/health", base_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .connect_timeout(HEALTH_TIMEOUT)
        .timeout(HEALTH_TIMEOUT)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client.get(url).send().await.map_err(|e| e.to_string())?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(format!("health returned {}", resp.status()))
    }
}

async fn stop_pid(pid: u32) {
    signal_pid(pid, libc::SIGTERM);
    let deadline = tokio::time::Instant::now() + SHUTDOWN_WAIT;
    while tokio::time::Instant::now() < deadline {
        if !process_alive(pid) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if process_alive(pid) {
        signal_pid(pid, libc::SIGKILL);
    }
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn signal_pid(pid: u32, signal: libc::c_int) {
    if pid == 0 {
        return;
    }
    let _ = unsafe { libc::kill(pid as libc::pid_t, signal) };
}

pub fn read_state(node_id: &str) -> Result<Option<TunnelState>, CliError> {
    let path = state_path(node_id)?;
    let Ok(content) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    let state = serde_json::from_str(&content)
        .map_err(|e| CliError::Parse(format!("parse tunnel state: {e}")))?;
    Ok(Some(state))
}

fn write_state(state: &TunnelState) -> Result<(), CliError> {
    let path = state_path(&state.node_id)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::Transport(format!("create tunnel state dir: {e}")))?;
    }
    let json = serde_json::to_string_pretty(state)
        .map_err(|e| CliError::Parse(format!("serialize tunnel state: {e}")))?;
    std::fs::write(path, json).map_err(|e| CliError::Transport(format!("write tunnel state: {e}")))
}

fn remove_state(node_id: &str) -> Result<(), CliError> {
    let path = state_path(node_id)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(CliError::Transport(format!("remove tunnel state: {e}"))),
    }
}

fn state_path(node_id: &str) -> Result<PathBuf, CliError> {
    let home = dirs::home_dir()
        .ok_or_else(|| CliError::InvalidConfig("home directory not available".to_string()))?;
    let encoded = utf8_percent_encode(node_id.trim(), NON_ALPHANUMERIC).to_string();
    Ok(home
        .join(".gitim/fleet-tunnels")
        .join(format!("{encoded}.json")))
}

fn tunnel_log_path(node_id: &str) -> PathBuf {
    let encoded = utf8_percent_encode(node_id.trim(), NON_ALPHANUMERIC).to_string();
    crate::daemon_log::logs_dir().join(format!("fleet-tunnel-{encoded}.log"))
}
