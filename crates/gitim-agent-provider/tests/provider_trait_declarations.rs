//! Pin every provider's `reports_usage()` and `usage_is_cumulative()`
//! declarations so accidental drift in defaults shows up here, not in the
//! token statistics layer where the symptom is silent under-counting.

use gitim_agent_provider::{
    claude::ClaudeProvider, codex::CodexProvider, gemini::GeminiProvider, hermes::HermesProvider,
    mock::MockProvider, openclaw::OpenclawProvider, opencode::OpencodeProvider, pi::PiProvider,
    Provider, ProviderConfig,
};

fn cfg() -> ProviderConfig {
    ProviderConfig::default()
}

#[test]
fn claude_reports_per_turn_increments() {
    let p = ClaudeProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
}

#[test]
fn codex_reports_cumulative_session_usage() {
    // Codex CLI 0.130.0-alpha.5 stdout emits exactly one `turn.completed`
    // per `codex exec` invocation, with `usage.input_tokens` being the
    // running session total. Verified by resume probe:
    //   turn1 input=22834 → turn2 input=45700 → turn3 input=68580.
    // The runtime's `normalize_to_delta` subtracts the per-session
    // baseline (stored in AgentState.last_session_usage) to recover the
    // per-turn delta for the accumulator. The HUD must not use raw cumulative
    // input as occupancy; Codex supplies current context separately from the
    // rollout token_count path when available.
    let p = CodexProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(p.usage_is_cumulative());
}

#[test]
fn opencode_reports_per_turn_increments() {
    let p = OpencodeProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
}

#[test]
fn pi_reports_per_turn_increments() {
    let p = PiProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
}

#[test]
fn hermes_reports_session_cumulative_and_self_manages_context() {
    // The hermes ACP id=3 prompt response carries session-cumulative usage
    // straight out of `run_agent.py`'s `session_input_tokens += ...`
    // accumulation, so `usage_is_cumulative = true` is the honest
    // declaration — `normalize_to_delta`'s baseline subtraction turns
    // those running totals into per-turn deltas for the accumulator.
    //
    // The dual `self_managed_context = true` is what keeps those
    // cumulative numbers out of `compute_snapshot`'s occupancy gauge:
    // hermes' own `compression.threshold: 0.5` is the only context-
    // pressure valve, the runtime gauge would lie regardless of which
    // (cumulative or per-call) shape it received.
    let p = HermesProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(p.usage_is_cumulative());
    assert!(p.self_managed_context());
}

#[test]
fn gemini_does_not_report_usage() {
    let p = GeminiProvider::new(cfg());
    assert!(!p.reports_usage());
}

#[test]
fn openclaw_does_not_report_usage() {
    let p = OpenclawProvider::new(cfg());
    assert!(!p.reports_usage());
}

#[test]
fn mock_defaults_match_typical_provider() {
    let p = MockProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
}

#[test]
fn mock_setters_propagate_to_trait() {
    let p = MockProvider::new(cfg())
        .with_reports_usage(false)
        .with_usage_is_cumulative(true);
    assert!(!p.reports_usage());
    assert!(p.usage_is_cumulative());
}
