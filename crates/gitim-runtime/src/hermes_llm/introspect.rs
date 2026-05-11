//! Runtime introspection of the user's hermes home directory.
//!
//! Reads `<hermes_home>/.env` and `<hermes_home>/config.yaml` to discover
//! which LLM providers are actually configured (i.e. have an API key present),
//! and returns a flat list of [`LlmProvider`] records for downstream use.

use std::path::Path;

use serde::{Deserialize, Serialize};

use super::registry::{ApiProtocol, BuiltinProvider, BUILTIN_PROVIDERS};

// ─── Public types ────────────────────────────────────────────────────────────

/// Whether the provider requires a user-supplied API key or is a custom
/// endpoint the user added manually via `config.yaml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    ApiKey,
    Custom,
}

/// A single LLM provider that is available in the user's hermes home.
///
/// `api_protocol` is copied from the registry so that downstream code (e.g.
/// `fetch_models`) can decide which wire protocol to speak without a separate
/// registry lookup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProvider {
    pub id: String,
    pub label: String,
    pub kind: ProviderKind,
    pub base_url: Option<String>,
    pub api_protocol: ApiProtocol,
}

// ─── Public functions ─────────────────────────────────────────────────────────

/// Discover which LLM providers are configured in `hermes_home`.
///
/// Returns an empty `Vec` on any unrecoverable error (missing directory,
/// unreadable files). Individual sources (`.env`, `config.yaml`) are skipped
/// independently on failure.
///
/// Result ordering: built-in providers sorted alphabetically by `id`, then
/// custom providers in the order they appear in `config.yaml`.
pub fn list_providers(hermes_home: &Path) -> Vec<LlmProvider> {
    if !hermes_home.exists() {
        return Vec::new();
    }

    let env_vars = read_env_file(hermes_home);
    let mut providers: Vec<LlmProvider> = collect_builtins(&env_vars);
    providers.extend(collect_custom(hermes_home));
    providers
}

/// Return the provider catalog users can select from.
///
/// Built-in providers always appear. Entries with configured API keys carry
/// their resolved base URL, and custom providers are appended from
/// `config.yaml`.
pub fn list_selectable_providers(hermes_home: &Path) -> Vec<LlmProvider> {
    let configured_builtins = collect_builtins(&read_env_file(hermes_home));
    let mut configured_by_id: std::collections::HashMap<String, LlmProvider> = configured_builtins
        .into_iter()
        .map(|provider| (provider.id.clone(), provider))
        .collect();

    let mut providers = Vec::new();
    for provider in BUILTIN_PROVIDERS {
        if let Some(configured) = configured_by_id.remove(provider.id) {
            providers.push(configured);
        } else {
            providers.push(LlmProvider {
                id: provider.id.to_owned(),
                label: provider.label.to_owned(),
                kind: ProviderKind::ApiKey,
                base_url: Some(provider.base_url.to_owned()),
                api_protocol: provider.api_protocol,
            });
        }
    }

    providers.extend(collect_custom(hermes_home));
    providers
}

/// Resolve the effective base URL for a built-in provider given the env value
/// that matched one of its `env_vars`.
///
/// Currently the only special case is `kimi-coding`: when the API key starts
/// with `"sk-kimi-"`, the coding endpoint is used instead of the moonshot
/// default. All other providers return the registry default as-is.
pub(crate) fn resolve_builtin_base_url(provider: &BuiltinProvider, env_value: &str) -> String {
    if provider.id == "kimi-coding" && env_value.starts_with("sk-kimi-") {
        return "https://api.kimi.com/coding/v1".to_owned();
    }
    provider.base_url.to_owned()
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Parse `<hermes_home>/.env` into a map of KEY → value.
///
/// Format: one `KEY=VALUE` pair per line. Lines starting with `#` or that are
/// blank are skipped. Whitespace around the `=` separator is stripped. Values
/// are not unquoted — hermes writes bare values without surrounding quotes.
///
/// Returns an empty map when the file is missing or unreadable.
fn read_env_file(hermes_home: &Path) -> std::collections::HashMap<String, String> {
    let path = hermes_home.join(".env");
    let raw = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return std::collections::HashMap::new(),
    };
    // Strip UTF-8 BOM (U+FEFF) that some Windows tools prepend. Rust's trim()
    // only covers ASCII whitespace, so without this the first key becomes
    // "\u{FEFF}KEY" and silently fails provider detection.
    let content = raw.strip_prefix('\u{FEFF}').unwrap_or(&raw);

    let mut map = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once('=') {
            let key = key.trim().to_owned();
            let value = value.trim().to_owned();
            if !key.is_empty() {
                map.insert(key, value);
            }
        }
    }
    map
}

/// Walk `BUILTIN_PROVIDERS` in alphabetical order (the static slice is already
/// alphabetically sorted) and emit an `LlmProvider` for each entry that has at
/// least one env var present with a non-empty value.
fn collect_builtins(env_vars: &std::collections::HashMap<String, String>) -> Vec<LlmProvider> {
    let mut result = Vec::new();

    for provider in BUILTIN_PROVIDERS {
        // Find the first env var that has a non-empty value.
        let matched_value = provider
            .env_vars
            .iter()
            .filter_map(|var| {
                let val = env_vars.get(*var)?;
                if val.is_empty() {
                    None
                } else {
                    Some(val.as_str())
                }
            })
            .next();

        if let Some(env_value) = matched_value {
            let base_url = resolve_builtin_base_url(provider, env_value);
            result.push(LlmProvider {
                id: provider.id.to_owned(),
                label: provider.label.to_owned(),
                kind: ProviderKind::ApiKey,
                base_url: Some(base_url),
                api_protocol: provider.api_protocol,
            });
        }
    }

    result
}

/// Parse `<hermes_home>/config.yaml` and collect any custom provider entries.
///
/// Skips (with a `tracing::warn!`) on missing file, parse failure, or missing
/// field — callers must not rely on custom providers being present.
fn collect_custom(hermes_home: &Path) -> Vec<LlmProvider> {
    let path = hermes_home.join("config.yaml");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(), // File simply not present — silent skip.
    };

    #[derive(Deserialize)]
    struct ConfigYaml {
        #[serde(default)]
        custom_providers: Vec<CustomProviderEntry>,
    }

    #[derive(Deserialize)]
    struct CustomProviderEntry {
        name: String,
        base_url: Option<String>,
    }

    let config: ConfigYaml = match serde_yaml::from_str(&content) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("failed to parse {}: {e}", path.display());
            return Vec::new();
        }
    };

    config
        .custom_providers
        .into_iter()
        .map(|entry| LlmProvider {
            id: format!("custom:{}", entry.name),
            label: format!("{} (custom)", entry.name),
            kind: ProviderKind::Custom,
            base_url: entry.base_url,
            api_protocol: ApiProtocol::OpenAI,
        })
        .collect()
}

// ─── Unit tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hermes_llm::registry::BUILTIN_PROVIDERS;

    fn kimi_provider() -> &'static BuiltinProvider {
        BUILTIN_PROVIDERS
            .iter()
            .find(|p| p.id == "kimi-coding")
            .expect("kimi-coding must exist in registry")
    }

    #[test]
    fn resolve_kimi_sk_kimi_prefix() {
        let url = resolve_builtin_base_url(kimi_provider(), "sk-kimi-abc123");
        assert_eq!(url, "https://api.kimi.com/coding/v1");
    }

    #[test]
    fn resolve_kimi_other_prefix_returns_default() {
        let url = resolve_builtin_base_url(kimi_provider(), "mk-abc");
        assert_eq!(url, "https://api.moonshot.ai/v1");
    }

    #[test]
    fn resolve_non_kimi_provider_returns_registry_url() {
        let deepseek = BUILTIN_PROVIDERS
            .iter()
            .find(|p| p.id == "deepseek")
            .expect("deepseek");
        let url = resolve_builtin_base_url(deepseek, "sk-deep-something");
        assert_eq!(url, "https://api.deepseek.com/v1");
    }
}
