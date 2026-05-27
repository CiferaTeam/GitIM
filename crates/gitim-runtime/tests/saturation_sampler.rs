//! End-to-end integration test for SaturationSampler::spawn lifecycle.
//! Mock RuntimeState with agents, spawn sampler at 100ms interval,
//! wait, and verify disk files reflect the working flag changes.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use gitim_runtime::http::{AgentInfo, RuntimeState, SharedRuntimeState};
use gitim_runtime::saturation_log::AgentSaturationLog;
use gitim_runtime::saturation_sampler::SaturationSampler;
use gitim_runtime::workspace::WorkspaceContext;
use tempfile::TempDir;
use tokio::sync::broadcast;

fn make_agent(handler: &str, working_flag: Arc<AtomicBool>) -> AgentInfo {
    AgentInfo {
        id: handler.to_string(),
        handler: handler.to_string(),
        display_name: handler.to_string(),
        status: "running".into(),
        last_activity: None,
        messages_processed: 0,
        repo_path: String::new(),
        provider: None,
        model: None,
        system_prompt: None,
        introduction: None,
        env: HashMap::new(),
        error_message: None,
        session_usage: None,
        llm_provider: None,
        llm_model: None,
        usage_summary: None,
        saturation_summary: None,
        is_working: working_flag,
        loop_handle: None,
    }
}

fn make_state(workspace_root: std::path::PathBuf, agents: Vec<AgentInfo>) -> SharedRuntimeState {
    let (tx, _) = broadcast::channel(16);
    let mut agent_map = HashMap::new();
    for a in agents {
        agent_map.insert(a.id.clone(), a);
    }
    let ctx = WorkspaceContext {
        slug: "test".into(),
        workspace_name: "test".into(),
        path: workspace_root,
        human_repo: None,
        poll_cursor: None,
        agents: agent_map,
        activity_tx: tx,
        auth_failed: Arc::new(AtomicBool::new(false)),
        git_config: None,
    };
    let mut rs = RuntimeState::default();
    rs.workspaces.insert("test".into(), ctx);
    Arc::new(Mutex::new(rs))
}

#[tokio::test]
async fn spawned_sampler_writes_disk_after_one_tick() {
    let dir = TempDir::new().unwrap();
    let alice_flag = Arc::new(AtomicBool::new(true));
    let bob_flag = Arc::new(AtomicBool::new(false));
    let state = make_state(
        dir.path().to_path_buf(),
        vec![
            make_agent("alice", alice_flag.clone()),
            make_agent("bob", bob_flag.clone()),
        ],
    );

    // 100ms interval: spawn skips the first tick, so we need to wait
    // ~250ms to guarantee one real tick lands.
    let _shutdown =
        SaturationSampler::with_interval(state.clone(), Duration::from_millis(100)).spawn();
    tokio::time::sleep(Duration::from_millis(250)).await;

    let alice = AgentSaturationLog::load_or_default(dir.path(), "alice");
    let bob = AgentSaturationLog::load_or_default(dir.path(), "bob");
    assert!(
        alice.totals.total_samples >= 1,
        "alice should have at least one sample, got {}",
        alice.totals.total_samples
    );
    assert_eq!(
        alice.totals.working_samples, alice.totals.total_samples,
        "alice flag was true the entire run"
    );
    assert!(
        bob.totals.total_samples >= 1,
        "bob should have at least one sample, got {}",
        bob.totals.total_samples
    );
    assert_eq!(
        bob.totals.working_samples, 0,
        "bob flag was false the entire run"
    );
}

#[tokio::test]
async fn flag_changes_reflect_in_subsequent_ticks() {
    let dir = TempDir::new().unwrap();
    let alice_flag = Arc::new(AtomicBool::new(false));
    let state = make_state(
        dir.path().to_path_buf(),
        vec![make_agent("alice", alice_flag.clone())],
    );
    let _shutdown =
        SaturationSampler::with_interval(state.clone(), Duration::from_millis(100)).spawn();

    // Wait for first tick (skip + one real tick).
    tokio::time::sleep(Duration::from_millis(250)).await;
    let baseline = AgentSaturationLog::load_or_default(dir.path(), "alice");
    let baseline_total = baseline.totals.total_samples;

    // Flip flag and wait for at least one more tick.
    alice_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    tokio::time::sleep(Duration::from_millis(200)).await;

    let after = AgentSaturationLog::load_or_default(dir.path(), "alice");
    assert!(
        after.totals.total_samples > baseline_total,
        "expected new samples since baseline ({} → {})",
        baseline_total,
        after.totals.total_samples
    );
    assert!(
        after.totals.working_samples > 0,
        "expected at least one working sample after flip, got 0"
    );
}
