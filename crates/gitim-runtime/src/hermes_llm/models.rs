//! Live HTTP fetch of the `/models` endpoint for a given LLM provider.
//!
//! # Errors returned via `ModelListResult.error`
//!
//! All errors are structured as human-readable strings suitable for display in
//! the WebUI. API keys are NEVER included in error strings — they travel only
//! in the `Authorization` request header and are discarded after use.
//!
//! | Situation | `error` value |
//! |-----------|---------------|
//! | Anthropic-protocol provider | "... uses Anthropic protocol; /models not supported. Use Custom..." |
//! | Missing `base_url` | "missing base_url for provider <id>" |
//! | Missing API key | "missing api key for <id> — set <ENV_VAR> in ~/.hermes/.env" |
//! | Network / DNS error | "network error: {e}" |
//! | Timeout | "timeout fetching /models for <id>" |
//! | HTTP 401 / 403 | "auth failed (HTTP {code}) — verify api key" |
//! | Other HTTP 4xx / 5xx | "upstream HTTP {code}" |
//! | JSON parse failure | "unexpected response schema (not OpenAI-compatible) — use Custom..." |
//! | Missing / non-array `data` | same as JSON parse failure |

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::introspect::ProviderKind;
use super::registry::ApiProtocol;
use super::LlmProvider;

// ─── Public types ────────────────────────────────────────────────────────────

/// A single model entry from the `/models` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
}

/// Result of a `fetch_models` call.
///
/// `custom_allowed` is always `true` (spec L1): the UI always shows the
/// "custom model ID" input regardless of whether live model fetch succeeds.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelListResult {
    pub models: Vec<ModelEntry>,
    pub custom_allowed: bool,
    pub error: Option<String>,
    pub fetched_at_ms: u64,
}

impl ModelListResult {
    fn ok(models: Vec<ModelEntry>) -> Self {
        Self {
            models,
            custom_allowed: true,
            error: None,
            fetched_at_ms: now_ms(),
        }
    }

    fn err(msg: impl Into<String>) -> Self {
        Self {
            models: vec![],
            custom_allowed: true,
            error: Some(msg.into()),
            fetched_at_ms: now_ms(),
        }
    }
}

// ─── Public function ─────────────────────────────────────────────────────────

/// Fetch the list of available models for `provider` using its `/models` endpoint.
///
/// Reads API keys from `<hermes_home>/.env` (ApiKey providers) or
/// `<hermes_home>/config.yaml` (Custom providers). Never retries. Times out
/// after 5 seconds.
///
/// Returns a `ModelListResult` — always succeeds structurally, errors are
/// embedded in `.error`.
pub async fn fetch_models(provider: &LlmProvider, hermes_home: &Path) -> ModelListResult {
    // ── 1. Anthropic-protocol short-circuit ──────────────────────────────────
    if provider.api_protocol == ApiProtocol::Anthropic {
        return ModelListResult::err(format!(
            "{} uses Anthropic protocol; /models not supported. Use Custom model input instead.",
            provider.id
        ));
    }

    // ── 2. Resolve base URL ──────────────────────────────────────────────────
    let base_url = match &provider.base_url {
        Some(u) => u.trim_end_matches('/').to_owned(),
        None => {
            return ModelListResult::err(format!(
                "missing base_url for provider {}",
                provider.id
            ));
        }
    };

    // ── 3. Resolve API key ───────────────────────────────────────────────────
    let api_key = match resolve_api_key(provider, hermes_home) {
        Some(k) => k,
        None => {
            // Build a hint about which env var to set.
            let hint = env_var_hint(provider);
            return ModelListResult::err(format!(
                "missing api key for {} — set {} in ~/.hermes/.env",
                provider.id, hint
            ));
        }
    };

    // ── 4. Build reqwest client ──────────────────────────────────────────────
    let client = match reqwest::ClientBuilder::new()
        .timeout(std::time::Duration::from_secs(5))
        .build()
    {
        Ok(c) => c,
        Err(e) => return ModelListResult::err(format!("network error: {e}")),
    };

    // ── 5. GET <base_url>/models ─────────────────────────────────────────────
    let url = format!("{base_url}/models");
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            // reqwest encodes timeout as a kind::Timeout error.
            if e.is_timeout() {
                return ModelListResult::err(format!(
                    "timeout fetching /models for {}",
                    provider.id
                ));
            }
            return ModelListResult::err(format!("network error: {e}"));
        }
    };

    // ── 6. Error categorization by HTTP status ───────────────────────────────
    let status = response.status();
    if !status.is_success() {
        let code = status.as_u16();
        let err_msg = if code == 401 || code == 403 {
            format!("auth failed (HTTP {code}) — verify api key")
        } else {
            format!("upstream HTTP {code}")
        };
        return ModelListResult::err(err_msg);
    }

    // ── 7. Parse response schema ─────────────────────────────────────────────
    // Use `.bytes()` first so the 5s client timeout covers body reading too
    // (reqwest drains the body, triggering the timeout on slow servers).
    // See: github.rs `send_github_get` — same pattern.
    let raw_bytes = match response.bytes().await {
        Ok(b) => b,
        Err(e) => {
            if e.is_timeout() {
                return ModelListResult::err(format!(
                    "timeout fetching /models for {}",
                    provider.id
                ));
            }
            return ModelListResult::err(format!("network error: {e}"));
        }
    };

    let body: serde_json::Value = match serde_json::from_slice(&raw_bytes) {
        Ok(v) => v,
        Err(_) => {
            return ModelListResult::err(
                "unexpected response schema (not OpenAI-compatible) — use Custom model input instead."
            );
        }
    };

    let data = match body.get("data").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            return ModelListResult::err(
                "unexpected response schema (not OpenAI-compatible) — use Custom model input instead."
            );
        }
    };

    let models: Vec<ModelEntry> = data
        .iter()
        .filter_map(|item| {
            let id = item.get("id")?.as_str()?;
            Some(ModelEntry {
                id: id.to_owned(),
                label: id.to_owned(),
            })
        })
        .collect();

    ModelListResult::ok(models)
}

// ─── Private helpers ──────────────────────────────────────────────────────────

/// Resolve the API key for a provider from `hermes_home`.
///
/// - `ApiKey` providers: scan `<hermes_home>/.env` for any of the provider's
///   `env_vars` aliases that has a non-empty value.
/// - `Custom` providers: parse `<hermes_home>/config.yaml`, find the entry
///   whose `name` matches the suffix after `"custom:"` in `provider.id`, and
///   read its `api_key` field.
///
/// Returns `None` when no key is found (not when the file is missing).
fn resolve_api_key(provider: &LlmProvider, hermes_home: &Path) -> Option<String> {
    match provider.kind {
        ProviderKind::ApiKey => resolve_api_key_from_env(provider, hermes_home),
        ProviderKind::Custom => resolve_api_key_from_config(provider, hermes_home),
    }
}

fn resolve_api_key_from_env(provider: &LlmProvider, hermes_home: &Path) -> Option<String> {
    // We need the env_vars list from the registry to know which vars to scan.
    // LlmProvider doesn't carry env_vars, so look up the matching builtin.
    use super::registry::BUILTIN_PROVIDERS;

    let env_vars: &[&str] = BUILTIN_PROVIDERS
        .iter()
        .find(|b| b.id == provider.id)
        .map(|b| b.env_vars)
        .unwrap_or(&[]);

    if env_vars.is_empty() {
        return None;
    }

    let env_path = hermes_home.join(".env");
    let content = std::fs::read_to_string(&env_path).ok()?;

    // Parse the .env file into key=value pairs.
    let map: std::collections::HashMap<&str, &str> = content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            Some((k.trim(), v.trim()))
        })
        .collect();

    // Return the first non-empty value among the provider's env_vars.
    for var in env_vars {
        if let Some(&val) = map.get(var) {
            if !val.is_empty() {
                return Some(val.to_owned());
            }
        }
    }

    None
}

fn resolve_api_key_from_config(provider: &LlmProvider, hermes_home: &Path) -> Option<String> {
    // Strip "custom:" prefix to get the name field used in config.yaml.
    let name = provider.id.strip_prefix("custom:")?;

    let config_path = hermes_home.join("config.yaml");
    let content = std::fs::read_to_string(&config_path).ok()?;

    #[derive(Deserialize)]
    struct ConfigYaml {
        #[serde(default)]
        custom_providers: Vec<CustomEntry>,
    }

    #[derive(Deserialize)]
    struct CustomEntry {
        name: String,
        api_key: Option<String>,
    }

    let config: ConfigYaml = serde_yaml::from_str(&content).ok()?;
    let entry = config.custom_providers.into_iter().find(|e| e.name == name)?;
    let key = entry.api_key.filter(|k| !k.is_empty())?;
    Some(key)
}

/// Build a user-friendly hint about which env var to set.
///
/// For built-in providers: the primary env var (first in the list).
/// For custom providers: just show the custom ID.
fn env_var_hint(provider: &LlmProvider) -> String {
    use super::registry::BUILTIN_PROVIDERS;

    if let Some(builtin) = BUILTIN_PROVIDERS.iter().find(|b| b.id == provider.id) {
        if let Some(first) = builtin.env_vars.first() {
            return first.to_string();
        }
    }

    format!("{} api_key in config.yaml", provider.id)
}

/// Current time as milliseconds since UNIX epoch.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
