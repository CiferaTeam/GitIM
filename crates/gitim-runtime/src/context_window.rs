//! Context window tracking: per-provider default budgets and tokenizer helpers.

use std::sync::OnceLock;
use tiktoken_rs::{cl100k_base, o200k_base, CoreBPE};

pub const WARN_AT_PERCENT: f64 = 80.0;

/// Default max-context-tokens for the given provider/model pair.
///
/// Returns `None` when the provider reports `used_percent` directly and no
/// token count is meaningful at the runtime layer (currently: Codex).
pub fn default_max_tokens(provider: &str, model: &str) -> Option<u64> {
    match provider {
        "claude" => {
            if model.contains("opus-4-7") && model.contains("1m") {
                Some(1_000_000)
            } else {
                Some(200_000)
            }
        }
        "codex" => None,
        "mock" => Some(10_000),
        _ => Some(200_000),
    }
}

#[cfg(test)]
mod default_max_tests {
    use super::*;

    #[test]
    fn claude_sonnet_defaults_to_200k() {
        assert_eq!(default_max_tokens("claude", "claude-sonnet-4-6"), Some(200_000));
    }

    #[test]
    fn claude_opus_1m_variant_defaults_to_1m() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7[1m]"), Some(1_000_000));
    }

    #[test]
    fn claude_opus_default_is_200k() {
        assert_eq!(default_max_tokens("claude", "claude-opus-4-7"), Some(200_000));
    }

    #[test]
    fn codex_returns_none() {
        assert_eq!(default_max_tokens("codex", "gpt-5"), None);
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
