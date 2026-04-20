//! Context window tracking: per-provider default budgets and tokenizer helpers.

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
