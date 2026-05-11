//! End-to-end coverage for the agent token usage statistics pipeline:
//!
//!   provider → agent_loop::update_session_usage
//!     → AgentState.last_session_usage (normalize)
//!     → AgentUsageLog (accumulate + persist)
//!     → AgentInfo.usage_summary patched in-memory
//!     → SSE "usage" event with sibling payload
//!
//! These tests bypass the live polling loop and drive `update_session_usage`
//! directly, since the goal is to lock the wiring rather than re-test
//! provider streaming. The tests verify both incremental and cumulative
//! provider semantics through the `replace_provider_for_test` seam.

use std::sync::{Arc, Mutex};

use gitim_agent_provider::{mock::MockProvider, ProviderConfig, ProviderUsage};
use gitim_runtime::agent_loop::AgentLoop;
use gitim_runtime::http::{AgentInfo, RuntimeState, SharedRuntimeState};
use gitim_runtime::state::AgentState;
use gitim_runtime::usage_log::AgentUsageLog;
use gitim_runtime::workspace::WorkspaceContext;

const SLUG: &str = "ws-usage";
const HANDLER: &str = "alice";

/// Build a wired-up AgentLoop + RuntimeState whose ctx.agents map already
/// contains a fresh AgentInfo for HANDLER. Returns the loop, the shared
/// state, and the tempdir so the caller keeps it alive until assertions
/// finish (dropping the tempdir wipes the on-disk usage log).
fn harness(
    provider: Box<dyn gitim_agent_provider::Provider>,
) -> (AgentLoop, SharedRuntimeState, tempfile::TempDir) {
    let tmp = tempfile::TempDir::new().expect("tempdir");
    let workspace_root = tmp.path().to_path_buf();
    // Agent clone path — `AgentLoop::with_provider` reads me.json from here.
    let agent_clone = workspace_root.join(HANDLER);
    let gitim_dir = agent_clone.join(".gitim");
    std::fs::create_dir_all(&gitim_dir).expect("mkdir clone .gitim");
    std::fs::write(
        gitim_dir.join("me.json"),
        format!("{{\"handler\":\"{HANDLER}\"}}"),
    )
    .expect("write me.json");

    let mut loop_ = AgentLoop::with_provider(&agent_clone, "mock", HANDLER).expect("agent loop");
    loop_.replace_provider_for_test(provider);

    let mut ctx = WorkspaceContext::new(SLUG.to_string(), SLUG.to_string(), workspace_root.clone());
    let activity_tx = ctx.activity_tx.clone();
    ctx.agents.insert(
        HANDLER.to_string(),
        AgentInfo {
            id: HANDLER.to_string(),
            handler: HANDLER.to_string(),
            display_name: HANDLER.to_string(),
            status: "running".to_string(),
            last_activity: None,
            messages_processed: 0,
            repo_path: agent_clone.display().to_string(),
            provider: Some("mock".to_string()),
            model: None,
            system_prompt: None,
            introduction: None,
            env: Default::default(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            loop_handle: None,
        },
    );

    let state: SharedRuntimeState = Arc::new(Mutex::new(RuntimeState::default()));
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(SLUG.to_string(), ctx);

    loop_.set_runtime_state(state.clone());
    loop_.set_activity_tx_with_workspace(activity_tx, SLUG.to_string());
    loop_.set_workspace_root(workspace_root);

    (loop_, state, tmp)
}

fn turn(input: u64, output: u64, cache_read: u64, cache_creation: u64) -> ProviderUsage {
    ProviderUsage {
        input_tokens: Some(input),
        output_tokens: Some(output),
        cache_read_tokens: Some(cache_read),
        cache_creation_tokens: Some(cache_creation),
        used_percent: None,
    }
}

#[test]
fn incremental_provider_accumulates_each_turn_directly() {
    let provider = Box::new(MockProvider::new(ProviderConfig::default()));
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();

    loop_
        .update_session_usage(&mut agent_state, Some(&turn(100, 50, 1000, 10)), "sess-A")
        .unwrap();
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(200, 75, 2000, 20)), "sess-A")
        .unwrap();

    // In-memory state must reflect the sum of both turns.
    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().expect("summary patched");
    assert_eq!(summary.totals.input, 300);
    assert_eq!(summary.totals.output, 125);
    assert_eq!(summary.totals.cache_read, 3000);
    assert_eq!(summary.totals.cache_creation, 30);
    assert_eq!(summary.totals.turns, 2);
    drop(s);

    // Disk must agree with in-memory (recovery would resume from this).
    let log =
        AgentUsageLog::load_or_default(tmp.path(), HANDLER, "mock", "", true);
    assert_eq!(log.totals.input, 300);
    assert_eq!(log.totals.turns, 2);
}

#[test]
fn cumulative_provider_subtracts_baseline_per_turn() {
    let provider = Box::new(MockProvider::new(ProviderConfig::default()).with_usage_is_cumulative(true));
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();

    // Codex-style: each turn reports the running session total.
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(100, 50, 1000, 10)), "sess-A")
        .unwrap();
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(300, 125, 3000, 30)), "sess-A")
        .unwrap();

    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().expect("summary patched");
    // Totals == final cumulative report (not double-counted).
    assert_eq!(summary.totals.input, 300);
    assert_eq!(summary.totals.output, 125);
    assert_eq!(summary.totals.cache_read, 3000);
    assert_eq!(summary.totals.cache_creation, 30);
    assert_eq!(summary.totals.turns, 2);
}

#[test]
fn cumulative_provider_resets_baseline_on_session_id_change() {
    let provider = Box::new(MockProvider::new(ProviderConfig::default()).with_usage_is_cumulative(true));
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();

    // Session A accumulates to 300/125.
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(300, 125, 0, 0)), "sess-A")
        .unwrap();

    // Session B starts a fresh cumulative count. The baseline must reset
    // to zero — otherwise the first turn of B would be "subtracted" against
    // A's terminal value and we'd record a giant negative-clamped delta.
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(80, 40, 0, 0)), "sess-B")
        .unwrap();

    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().unwrap();
    // 300 (final A) + 80 (B's first cumulative report after baseline reset)
    assert_eq!(summary.totals.input, 380);
    assert_eq!(summary.totals.output, 165);
    assert_eq!(summary.totals.turns, 2);
}

#[test]
fn cumulative_provider_saturates_when_cache_read_regresses() {
    // Anthropic's prompt cache invalidates upstream, so cache_read can drop
    // mid-session. The normalizer must use saturating_sub instead of
    // panicking or wrapping; the regression is logged via tracing::warn but
    // the bucket only ever advances.
    let provider = Box::new(MockProvider::new(ProviderConfig::default()).with_usage_is_cumulative(true));
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();

    loop_
        .update_session_usage(&mut agent_state, Some(&turn(100, 50, 5000, 0)), "sess-A")
        .unwrap();
    // Cache invalidation upstream — cache_read drops from 5000 to 1000.
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(200, 100, 1000, 0)), "sess-A")
        .unwrap();

    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().unwrap();
    // input/output advance normally (cumulative ⇒ delta = 100/50).
    assert_eq!(summary.totals.input, 200);
    assert_eq!(summary.totals.output, 100);
    // cache_read does NOT regress; second turn contributes saturating_sub(1000, 5000) = 0.
    assert_eq!(summary.totals.cache_read, 5000);
}

#[test]
fn provider_without_usage_only_advances_turns() {
    // gemini / openclaw: reports_usage() == false. The accumulator must
    // count turns so we have a liveness signal, but never touch the token
    // counters even when (degenerate) usage data is supplied.
    let provider = Box::new(
        MockProvider::new(ProviderConfig::default()).with_reports_usage(false),
    );
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();

    // Even if the provider hands us a non-None ProviderUsage, the
    // reports_usage()=false declaration must short-circuit the token path.
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(9999, 9999, 9999, 9999)), "sess-A")
        .unwrap();
    loop_
        .update_session_usage(&mut agent_state, None, "sess-A")
        .unwrap();

    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().unwrap();
    assert_eq!(summary.totals.input, 0);
    assert_eq!(summary.totals.output, 0);
    assert_eq!(summary.totals.cache_read, 0);
    assert_eq!(summary.totals.turns, 2);
    assert!(!summary.provider_reports_usage);
}

#[test]
fn usage_summary_today_window_has_thirty_entries() {
    let provider = Box::new(MockProvider::new(ProviderConfig::default()));
    let (loop_, state, tmp) = harness(provider);
    let agent_clone = tmp.path().join(HANDLER);

    let mut agent_state = AgentState::load(&agent_clone).unwrap();
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(10, 5, 0, 0)), "sess-A")
        .unwrap();

    let s = state.lock().unwrap();
    let info = s.workspaces[SLUG].agents.get(HANDLER).unwrap();
    let summary = info.usage_summary.as_ref().unwrap();
    assert_eq!(
        summary.by_day.len(),
        30,
        "by_day window must always be 30 entries (zero-filled)"
    );
    let last = summary.by_day.last().unwrap();
    assert_eq!(last.bucket.input, 10);
}

#[test]
fn save_failure_increments_runtime_counter() {
    // Force the persistence path to fail by making the workspace root a
    // file (so .gitim-runtime/usage/ creation cannot succeed). The agent
    // loop must still complete — token statistics is non-critical — and
    // the failure counter must advance.
    let provider = Box::new(MockProvider::new(ProviderConfig::default()));
    let (mut loop_, state, _tmp) = harness(provider);

    let bad_root = std::env::temp_dir().join(format!("usage-e2e-bad-{}", std::process::id()));
    // Create a *file* at the path the loop will treat as the workspace root.
    std::fs::write(&bad_root, b"").unwrap();
    loop_.set_workspace_root(bad_root.clone());

    let agent_clone = _tmp.path().join(HANDLER);
    let mut agent_state = AgentState::load(&agent_clone).unwrap();
    loop_
        .update_session_usage(&mut agent_state, Some(&turn(1, 1, 0, 0)), "sess-A")
        .expect("update_session_usage must not bubble the save error");

    let s = state.lock().unwrap();
    assert!(
        s.usage_save_failures
            .load(std::sync::atomic::Ordering::Relaxed)
            >= 1,
        "save failure must bump the runtime counter"
    );

    // Cleanup the bogus path.
    let _ = std::fs::remove_file(&bad_root);
}
