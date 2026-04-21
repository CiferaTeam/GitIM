//! Context window tracking: per-provider default budgets and tokenizer helpers.

use std::sync::OnceLock;
use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};

pub const WARN_AT_PERCENT: f64 = 80.0;

/// Default max-context-tokens for the given provider/model pair.
///
/// This is a conservative lookup used as denominator for the usage percentage
/// when the provider doesn't report `used_percent` directly. The table leans
/// on published Anthropic / OpenAI documentation as of 2026-04; when a model
/// name is ambiguous we pick the smaller value so the bar fills faster rather
/// than showing a false sense of headroom.
///
/// A `Some` result also unlocks the runtime-tiktoken fallback: when the
/// provider reports nothing at all we still render a usage bar by dividing
/// our own tokenizer estimate by this value.
pub fn default_max_tokens(provider: &str, model: &str) -> Option<u64> {
    let m = model.to_ascii_lowercase();
    match provider {
        "claude" => Some(claude_max_tokens(&m)),
        "codex" => Some(codex_max_tokens(&m)),
        "mock" => Some(10_000),
        _ => Some(200_000),
    }
}

fn claude_max_tokens(model_lc: &str) -> u64 {
    // Explicit 1M opt-in marker (e.g. `claude-sonnet-4-6[1m]`,
    // `claude-opus-4-7[1m]`). Anthropic exposes the 1M window as a beta flag;
    // we surface it through the model string so per-agent config can pick it.
    if model_lc.contains("[1m]") || model_lc.contains("-1m") {
        return 1_000_000;
    }
    // Sonnet 4.5+ supports a 1M beta by default in Claude CLI ≥ 1.0.
    // sonnet-4-6 is the common default in WebUI; give it the full 1M.
    if model_lc.contains("sonnet-4-6") || model_lc.contains("sonnet-4-7") {
        return 1_000_000;
    }
    // Opus / Haiku / older Sonnet → classic 200k window.
    200_000
}

fn codex_max_tokens(model_lc: &str) -> u64 {
    // Codex CLI primarily targets gpt-5 / gpt-5-codex (272k effective context)
    // and o-series models. Unknown → conservative 272k so the estimate
    // fallback renders *something* rather than swallowing the snapshot.
    if model_lc.contains("gpt-5") || model_lc.contains("codex") {
        return 272_000;
    }
    if model_lc.contains("o1") || model_lc.contains("o3") {
        return 200_000;
    }
    if model_lc.contains("gpt-4.1") || model_lc.contains("gpt-4-1") {
        return 1_000_000;
    }
    272_000
}

#[cfg(test)]
mod default_max_tests {
    use super::*;

    #[test]
    fn claude_sonnet_4_6_is_1m() {
        assert_eq!(default_max_tokens("claude", "claude-sonnet-4-6"), Some(1_000_000));
    }

    #[test]
    fn claude_opus_1m_variant_is_1m() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7[1m]"), Some(1_000_000));
    }

    #[test]
    fn claude_opus_default_is_200k() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7"), Some(200_000));
    }

    #[test]
    fn claude_haiku_is_200k() {
        assert_eq!(default_max_tokens("claude", "claude-haiku-4-5"), Some(200_000));
    }

    #[test]
    fn claude_case_insensitive() {
        assert_eq!(default_max_tokens("claude", "Claude-Sonnet-4-6"), Some(1_000_000));
    }

    #[test]
    fn codex_gpt5_is_272k() {
        assert_eq!(default_max_tokens("codex", "gpt-5"), Some(272_000));
        assert_eq!(default_max_tokens("codex", "gpt-5-codex"), Some(272_000));
    }

    #[test]
    fn codex_o_series_is_200k() {
        assert_eq!(default_max_tokens("codex", "o1"), Some(200_000));
        assert_eq!(default_max_tokens("codex", "o3-mini"), Some(200_000));
    }

    #[test]
    fn codex_unknown_model_falls_back_to_272k() {
        // Key property: no longer returns None — estimate path will still
        // produce a snapshot instead of going dark (the "Codex 没数据" bug).
        assert_eq!(default_max_tokens("codex", "future-model"), Some(272_000));
    }

    #[test]
    fn mock_returns_10k() {
        assert_eq!(default_max_tokens("mock", "any"), Some(10_000));
    }

    #[test]
    fn unknown_provider_conservative_fallback() {
        assert_eq!(default_max_tokens("future", "some-model"), Some(200_000));
    }
}

static CL100K: OnceLock<CoreBPE> = OnceLock::new();
static O200K: OnceLock<CoreBPE> = OnceLock::new();

fn cl100k() -> Option<&'static CoreBPE> {
    match CL100K.get() {
        Some(b) => Some(b),
        None => cl100k_base().ok().map(|b| CL100K.get_or_init(|| b)),
    }
}

fn o200k() -> Option<&'static CoreBPE> {
    match O200K.get() {
        Some(b) => Some(b),
        None => o200k_base().ok().map(|b| O200K.get_or_init(|| b)),
    }
}

/// Count tokens in `text` using the encoder best suited for the given provider.
///
/// Returns 0 if the encoder fails to initialize (logged once by the caller) or
/// if the text is empty. Always succeeds for non-empty inputs once the encoder
/// is warm. First call per encoder pays ~100ms for BPE vocabulary load;
/// subsequent calls are O(n) over the input.
pub fn tokenize_for_provider(provider: &str, text: &str) -> u64 {
    if text.is_empty() {
        return 0;
    }
    let bpe = match provider {
        "codex" => o200k(),
        _ => cl100k(),
    };
    match bpe {
        Some(b) => b.encode_with_special_tokens(text).len() as u64,
        None => 0,
    }
}

#[cfg(test)]
mod tokenize_tests {
    use super::*;

    #[test]
    fn tokenize_claude_short_text() {
        let n = tokenize_for_provider("claude", "hello world");
        assert!(n > 0 && n < 20, "got {n}");
    }

    #[test]
    fn tokenize_codex_short_text() {
        let n = tokenize_for_provider("codex", "hello world");
        assert!(n > 0 && n < 20, "got {n}");
    }

    #[test]
    fn tokenize_empty_returns_zero() {
        assert_eq!(tokenize_for_provider("claude", ""), 0);
    }

    #[test]
    fn tokenize_same_text_is_stable() {
        let a = tokenize_for_provider("claude", "repeatable input");
        let b = tokenize_for_provider("claude", "repeatable input");
        assert_eq!(a, b);
    }
}
