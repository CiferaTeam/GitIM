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
fn codex_reports_per_turn_increments() {
    // Codex's `token_count.last_token_usage` is the per-LLM-call shape we
    // capture (overwriting earlier events in the same turn). normalize_to_delta
    // therefore adds it directly to the daily bucket — no baseline math.
    // Note: a codex turn that fires multiple LLM calls will only see the
    // final call's bill, which under-counts; full-fidelity billing would
    // require summing every `last_token_usage` event during the turn, not
    // just the most recent one. That's a separate billing-accuracy
    // improvement, not a context-window-occupancy concern.
    let p = CodexProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
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
