// Mirrored from hermes_cli/auth.py:PROVIDER_REGISTRY @ v0.10.0 (2026.4.16).
// Resync on hermes minor bumps; CI does not enforce.

/// Which wire protocol the provider speaks.
///
/// MiniMax series has no `/models` endpoint (Anthropic-protocol), so
/// `fetch_models` short-circuits based on this field rather than attempting
/// a list call that would always fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    OpenAI,
    Anthropic,
}

/// Static descriptor for a built-in LLM provider.
///
/// `base_url` is the default fallback; runtime introspect may override it
/// (e.g. kimi-coding switches to the coding endpoint when the API key prefix
/// indicates a kimi-native key).
#[derive(Debug, Clone, Copy)]
pub struct BuiltinProvider {
    pub id: &'static str,
    pub label: &'static str,
    pub env_vars: &'static [&'static str],
    pub base_url: &'static str,
    pub api_protocol: ApiProtocol,
}

/// All built-in providers, ordered alphabetically by `id`.
pub const BUILTIN_PROVIDERS: &[BuiltinProvider] = &[
    BuiltinProvider {
        id: "anthropic",
        label: "Anthropic / Claude",
        env_vars: &[
            "ANTHROPIC_API_KEY",
            "ANTHROPIC_TOKEN",
            "CLAUDE_CODE_OAUTH_TOKEN",
        ],
        base_url: "https://api.anthropic.com",
        api_protocol: ApiProtocol::OpenAI,
    },
    BuiltinProvider {
        id: "deepseek",
        label: "DeepSeek",
        env_vars: &["DEEPSEEK_API_KEY"],
        base_url: "https://api.deepseek.com/v1",
        api_protocol: ApiProtocol::OpenAI,
    },
    BuiltinProvider {
        id: "kimi-coding",
        label: "Kimi / Moonshot",
        env_vars: &["KIMI_API_KEY"],
        // Default fallback; introspect overrides to https://api.kimi.com/coding/v1
        // when KIMI_API_KEY starts with "sk-kimi-".
        base_url: "https://api.moonshot.ai/v1",
        api_protocol: ApiProtocol::OpenAI,
    },
    BuiltinProvider {
        id: "minimax",
        label: "MiniMax",
        env_vars: &["MINIMAX_API_KEY"],
        base_url: "https://api.minimax.io/anthropic",
        api_protocol: ApiProtocol::Anthropic,
    },
    BuiltinProvider {
        id: "minimax-cn",
        label: "MiniMax CN",
        env_vars: &["MINIMAX_CN_API_KEY"],
        base_url: "https://api.minimaxi.com/anthropic",
        api_protocol: ApiProtocol::Anthropic,
    },
    BuiltinProvider {
        id: "zai",
        label: "Z.AI / GLM",
        env_vars: &["GLM_API_KEY", "ZAI_API_KEY", "Z_AI_API_KEY"],
        base_url: "https://api.z.ai/api/paas/v4",
        api_protocol: ApiProtocol::OpenAI,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_six_builtin_providers() {
        assert_eq!(BUILTIN_PROVIDERS.len(), 6);
    }

    #[test]
    fn registry_ids_unique() {
        let mut ids: Vec<&str> = BUILTIN_PROVIDERS.iter().map(|p| p.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            BUILTIN_PROVIDERS.len(),
            "duplicate provider ids detected"
        );
    }

    #[test]
    fn registry_no_empty_env_vars() {
        for provider in BUILTIN_PROVIDERS {
            assert!(
                !provider.env_vars.is_empty(),
                "provider '{}' has no env_var aliases",
                provider.id
            );
        }
    }

    #[test]
    fn registry_anthropic_has_token_aliases() {
        let anthropic = BUILTIN_PROVIDERS
            .iter()
            .find(|p| p.id == "anthropic")
            .expect("anthropic provider not found");
        assert!(
            anthropic.env_vars.contains(&"ANTHROPIC_API_KEY"),
            "anthropic provider missing ANTHROPIC_API_KEY"
        );
        assert!(
            anthropic.env_vars.contains(&"ANTHROPIC_TOKEN"),
            "anthropic provider missing ANTHROPIC_TOKEN"
        );
        assert!(
            anthropic.env_vars.contains(&"CLAUDE_CODE_OAUTH_TOKEN"),
            "anthropic provider missing CLAUDE_CODE_OAUTH_TOKEN"
        );
    }

    #[test]
    fn registry_zai_has_glm_alias() {
        let zai = BUILTIN_PROVIDERS
            .iter()
            .find(|p| p.id == "zai")
            .expect("zai provider not found");
        assert!(
            zai.env_vars.contains(&"GLM_API_KEY"),
            "zai provider missing GLM_API_KEY alias"
        );
    }

    #[test]
    fn registry_minimax_protocols_anthropic() {
        for id in ["minimax", "minimax-cn"] {
            let provider = BUILTIN_PROVIDERS
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("provider '{}' not found", id));
            assert_eq!(
                provider.api_protocol,
                ApiProtocol::Anthropic,
                "provider '{}' should use Anthropic protocol",
                id
            );
        }
    }

    #[test]
    fn registry_others_protocol_openai() {
        for id in ["anthropic", "deepseek", "kimi-coding", "zai"] {
            let provider = BUILTIN_PROVIDERS
                .iter()
                .find(|p| p.id == id)
                .unwrap_or_else(|| panic!("provider '{}' not found", id));
            assert_eq!(
                provider.api_protocol,
                ApiProtocol::OpenAI,
                "provider '{}' should use OpenAI protocol",
                id
            );
        }
    }
}
