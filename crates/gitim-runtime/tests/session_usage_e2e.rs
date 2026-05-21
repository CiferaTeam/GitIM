#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Task 22: E2E smoke — synthetic provider usage flows through the agent
//! loop's snapshot path, lands on disk, and exposes the right shape.
//!
//! Approach: we drive `AgentLoop::update_session_usage` directly rather than
//! running a full `run_once` cycle. Running `run_once` end-to-end would
//! require a live daemon, a channel with messages, and a provider that
//! returns a non-None `session_token` — none of which MockProvider offers
//! today. The spec (Task 22) explicitly allows this shape: "A test that
//! exercises the computation + persistence path is more valuable than no
//! test."
//!
//! We still construct a real `AgentLoop` via the public `with_provider`
//! constructor so the provider_type / model / state-path wiring is exactly
//! what production uses. What we skip is the poll → format prompt → provider
//! round-trip, which the other E2E harnesses cover.

mod common;

use common::short_tempdir;

use gitim_agent_provider::ProviderUsage;
use gitim_runtime::agent_loop::AgentLoop;
use gitim_runtime::state::{AgentState, UsageSource};

/// Bootstrap a minimal agent workspace with just the files AgentLoop touches:
/// `.gitim/me.json` (so `with_defaults` could read a handler) and `.gitim/`
/// (so AgentState::save has a target dir).
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

/// Full-stack smoke: provider reports usage → runtime computes snapshot →
/// persists to `agent-state.json` → the snapshot carries the fields the
/// WebUI + SSE subscribers rely on.
#[tokio::test(flavor = "multi_thread")]
async fn usage_snapshot_surfaces_via_agent_state() {
    let tmp = bootstrap_workspace("e2e-agent");
    let repo_root = tmp.path();

    // Provider type "mock" gives us default_max_tokens = Some(10_000).
    // input_tokens = 5_000 → 50% (well below the 80% threshold).
    let usage = ProviderUsage {
        input_tokens: Some(5_000),
        output_tokens: Some(200),
        used_percent: None,
        ..Default::default()
    };

    // Real AgentLoop: goes through the `create("mock", ...)` factory,
    // loads state from disk (empty → default), wires provider_type/handler
    // the same way production does.
    let loop_ = AgentLoop::with_provider(repo_root, "mock", "e2e-agent").expect("loop");

    // Mirror what `run_once` does before calling update: bump the estimator
    // (pre-execute prompt tokens) then add assistant output. These counts
    // are what prove the estimator path ran — tiktoken returns non-zero for
    // non-empty text, so we can assert > 0 post-call.
    let mut state = AgentState::load(repo_root).expect("load state");
    state.estimated_tokens += gitim_runtime::context_window::tokenize_for_provider(
        "mock",
        "hello, world — pretend prompt",
    );
    state.estimated_tokens += gitim_runtime::context_window::tokenize_for_provider(
        "mock",
        "pretend assistant response text",
    );

    loop_
        .update_session_usage(&mut state, Some(&usage), "sess-e2e-token-abc")
        .expect("update usage");

    // Re-read from disk — verifies the persist path (not just in-memory state).
    let persisted = AgentState::load(repo_root).expect("reload state");

    let snap = persisted
        .session_usage
        .as_ref()
        .expect("session_usage persisted");

    assert_eq!(snap.session_id, "sess-e2e-token-abc");
    assert_eq!(snap.input_tokens, Some(5_000));
    assert_eq!(snap.output_tokens, Some(200));
    assert_eq!(snap.max_tokens, Some(10_000));
    assert!(
        (snap.used_percent - 50.0).abs() < 0.01,
        "expected 50%, got {}",
        snap.used_percent
    );
    assert!(matches!(snap.source, UsageSource::ProviderReported));
    assert!(
        !snap.updated_at.is_empty(),
        "updated_at should be populated from chrono::Utc::now"
    );

    // Estimator ran in parallel — we seeded it above, update shouldn't reset it.
    assert!(
        persisted.estimated_tokens > 0,
        "tiktoken path expected to have contributed non-zero tokens"
    );

    // 50% is well below the 80% threshold — no notice should be pending.
    assert!(
        !persisted.usage_notice_pending,
        "50% usage must not trip the 80% threshold"
    );
}
