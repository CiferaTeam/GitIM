//! Task 23: E2E crossing detection — two scripted turns at 55% then 82%
//! drive the 80% threshold state machine. Turn 1 stays silent; Turn 2 flips
//! `usage_notice_pending` to true.
//!
//! Scope note (see Task 23 "scale back" clause): we verify Turns 1 + 2
//! through the real `update_session_usage` path (detect → set flag →
//! persist). We do NOT verify Turn 3's prompt-prefix injection here,
//! because MockProvider does not currently capture the prompt it receives
//! and wiring up `Arc<Mutex<Vec<String>>>` capture for this single
//! assertion was scored not worth it. The preamble-content assertions
//! already live in `tests/agent_loop.rs` (`preamble_*` suite) and the
//! notice-consumption logic in `run_once` is covered by reading code —
//! manual verification recommended once this path is exercised by a
//! real provider.

mod common;

use common::short_tempdir;

use gitim_agent_provider::ProviderUsage;
use gitim_runtime::agent_loop::AgentLoop;
use gitim_runtime::state::AgentState;

fn bootstrap_workspace(handler: &str) -> tempfile::TempDir {
    let tmp = short_tempdir();
    let gitim_dir = tmp.path().join(".gitim");
    std::fs::create_dir_all(&gitim_dir).unwrap();
    std::fs::write(
        gitim_dir.join("me.json"),
        format!("{{\"handler\":\"{handler}\"}}"),
    )
    .unwrap();
    tmp
}

/// With mock's 10k max budget:
///   Turn 1: input_tokens = 5_500 → 55% → below threshold, flag stays false
///   Turn 2: input_tokens = 8_200 → 82% → crosses 80%, flag flips to true
///
/// The state machine's "only once" guarantee is already covered by the unit
/// tests in `tests/agent_loop.rs` (`not_crossed_when_already_above`), so we
/// focus here on the transition (false → true on first crossing).
#[tokio::test(flavor = "multi_thread")]
async fn threshold_crossing_sets_notice_pending() {
    let tmp = bootstrap_workspace("threshold-agent");
    let repo_root = tmp.path();

    let loop_ = AgentLoop::with_provider(repo_root, "mock", "threshold-agent").expect("loop");

    // ---- Turn 1: 55% ------------------------------------------------------
    let usage_t1 = ProviderUsage {
        input_tokens: Some(5_500),
        output_tokens: Some(200),
        used_percent: None,
        ..Default::default()
    };
    let mut state = AgentState::load(repo_root).expect("load");
    loop_
        .update_session_usage(&mut state, Some(&usage_t1), "sess-threshold-001")
        .expect("update t1");

    let after_t1 = AgentState::load(repo_root).expect("reload t1");
    let snap_t1 = after_t1.session_usage.as_ref().expect("snapshot t1");
    assert!(
        (snap_t1.used_percent - 55.0).abs() < 0.01,
        "t1 used_percent: {}",
        snap_t1.used_percent
    );
    assert!(
        !after_t1.usage_notice_pending,
        "55% must not trip the 80% threshold"
    );

    // ---- Turn 2: 82% -----------------------------------------------------
    // Reuse the session_id — `just_crossed_threshold` keys off the
    // previous snapshot's used_percent, which was persisted from turn 1.
    let usage_t2 = ProviderUsage {
        input_tokens: Some(8_200),
        output_tokens: Some(300),
        used_percent: None,
        ..Default::default()
    };
    let mut state = AgentState::load(repo_root).expect("load");
    loop_
        .update_session_usage(&mut state, Some(&usage_t2), "sess-threshold-001")
        .expect("update t2");

    let after_t2 = AgentState::load(repo_root).expect("reload t2");
    let snap_t2 = after_t2.session_usage.as_ref().expect("snapshot t2");
    assert!(
        (snap_t2.used_percent - 82.0).abs() < 0.01,
        "t2 used_percent: {}",
        snap_t2.used_percent
    );
    assert!(
        after_t2.usage_notice_pending,
        "crossing 80% must flip usage_notice_pending to true"
    );
}
