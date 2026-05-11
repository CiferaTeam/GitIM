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
fn codex_reports_session_cumulative() {
    let p = CodexProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(
        p.usage_is_cumulative(),
        "codex token_count event is session-cumulative; runtime relies on this"
    );
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
fn hermes_reports_per_turn_increments_for_now() {
    // Tracked under Task 2 — audit hermes result.usage shape and adjust if
    // it turns out to be cumulative. Default per-turn keeps `last_session_usage`
    // baseline at zero, which over-counts by zero (delta = current - 0 = current)
    // when the provider already sends per-turn deltas.
    let p = HermesProvider::new(cfg());
    assert!(p.reports_usage());
    assert!(!p.usage_is_cumulative());
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
