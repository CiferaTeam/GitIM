use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;

const DEFAULT_BIN_CODEX: &str = "codex";
const DEFAULT_BIN_OPENCODE: &str = "opencode";
const CATALOG_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCatalogResult {
    pub provider: String,
    pub source: String,
    pub supports_default: bool,
    pub supports_custom: bool,
    pub custom_format_hint: Option<String>,
    pub models: Vec<ModelOption>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ModelCatalogOverrides {
    pub codex_bin: Option<String>,
    pub opencode_bin: Option<String>,
}

#[async_trait]
pub trait ModelCatalogProvider {
    async fn list_models(&self) -> ModelCatalogResult;
}

pub async fn list_provider_models(provider: &str) -> ModelCatalogResult {
    list_provider_models_with_overrides(provider, ModelCatalogOverrides::default()).await
}

pub async fn list_provider_models_with_overrides(
    provider: &str,
    overrides: ModelCatalogOverrides,
) -> ModelCatalogResult {
    match provider {
        "codex" => {
            CodexCatalog {
                bin: overrides
                    .codex_bin
                    .unwrap_or_else(|| DEFAULT_BIN_CODEX.to_string()),
            }
            .list_models()
            .await
        }
        "opencode" => {
            OpenCodeCatalog {
                bin: overrides
                    .opencode_bin
                    .unwrap_or_else(|| DEFAULT_BIN_OPENCODE.to_string()),
            }
            .list_models()
            .await
        }
        "pi" => fallback_catalog(
            "pi",
            "pi_custom_model",
            true,
            true,
            Some("provider/model or model".to_string()),
            None,
        ),
        "claude" => fallback_catalog(
            "claude",
            "claude_custom_model",
            true,
            true,
            Some("model id accepted by claude --model".to_string()),
            None,
        ),
        "hermes" => fallback_catalog(
            "hermes",
            "hermes_llm_routes",
            true,
            false,
            None,
            Some("Hermes models are exposed through /hermes/llm providers routes".to_string()),
        ),
        other => fallback_catalog(
            other,
            "unknown_provider",
            false,
            false,
            None,
            Some(format!("unknown provider: {other}")),
        ),
    }
}

struct CodexCatalog {
    bin: String,
}

#[async_trait]
impl ModelCatalogProvider for CodexCatalog {
    async fn list_models(&self) -> ModelCatalogResult {
        match run_catalog_command(&self.bin, &["debug", "models"]).await {
            Ok(stdout) => match parse_codex_debug_models(&stdout) {
                Ok(models) => fallback_catalog(
                    "codex",
                    "codex_debug_models",
                    true,
                    true,
                    Some("model id accepted by codex --model".to_string()),
                    None,
                )
                .with_models(models),
                Err(error) => fallback_catalog(
                    "codex",
                    "codex_debug_models",
                    true,
                    true,
                    Some("model id accepted by codex --model".to_string()),
                    Some(error),
                ),
            },
            Err(error) => fallback_catalog(
                "codex",
                "codex_debug_models",
                true,
                true,
                Some("model id accepted by codex --model".to_string()),
                Some(error),
            ),
        }
    }
}

struct OpenCodeCatalog {
    bin: String,
}

#[async_trait]
impl ModelCatalogProvider for OpenCodeCatalog {
    async fn list_models(&self) -> ModelCatalogResult {
        match run_catalog_command(&self.bin, &["models"]).await {
            Ok(stdout) => fallback_catalog(
                "opencode",
                "opencode_models",
                true,
                true,
                Some("provider/model".to_string()),
                None,
            )
            .with_models(parse_opencode_models(&stdout)),
            Err(error) => fallback_catalog(
                "opencode",
                "opencode_models",
                true,
                true,
                Some("provider/model".to_string()),
                Some(error),
            ),
        }
    }
}

impl ModelCatalogResult {
    fn with_models(mut self, models: Vec<ModelOption>) -> Self {
        self.models = models;
        self
    }
}

fn fallback_catalog(
    provider: &str,
    source: &str,
    supports_default: bool,
    supports_custom: bool,
    custom_format_hint: Option<String>,
    error: Option<String>,
) -> ModelCatalogResult {
    ModelCatalogResult {
        provider: provider.to_string(),
        source: source.to_string(),
        supports_default,
        supports_custom,
        custom_format_hint,
        models: Vec::new(),
        error,
    }
}

pub fn parse_codex_debug_models(stdout: &str) -> Result<Vec<ModelOption>, String> {
    let root: serde_json::Value =
        serde_json::from_str(stdout).map_err(|e| format!("parse codex model JSON: {e}"))?;
    let models = root
        .get("models")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "codex model JSON missing models array".to_string())?;

    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for model in models {
        if model.get("visibility").and_then(|v| v.as_str()) != Some("list") {
            continue;
        }
        let Some(slug) = model.get("slug").and_then(|v| v.as_str()) else {
            continue;
        };
        if !seen.insert(slug.to_string()) {
            continue;
        }
        let label = model
            .get("display_name")
            .and_then(|v| v.as_str())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(slug);
        out.push(ModelOption {
            id: slug.to_string(),
            label: label.to_string(),
        });
    }

    Ok(out)
}

pub fn parse_opencode_models(stdout: &str) -> Vec<ModelOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for line in stdout.lines() {
        let id = line.trim();
        if id.is_empty() || !id.contains('/') || !seen.insert(id.to_string()) {
            continue;
        }
        out.push(ModelOption {
            id: id.to_string(),
            label: id.to_string(),
        });
    }
    out
}

async fn run_catalog_command(bin: &str, args: &[&str]) -> Result<String, String> {
    let mut child = tokio::process::Command::new(bin);
    child
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let output = match tokio::time::timeout(CATALOG_TIMEOUT, child.output()).await {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(format!("failed to spawn {bin}: {e}")),
        Err(_) => {
            return Err(format!(
                "{bin} model catalog exceeded {}ms",
                CATALOG_TIMEOUT.as_millis()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let trimmed = stderr.trim();
        let msg = if trimmed.is_empty() {
            format!("{bin} exited with status {}", output.status)
        } else {
            format!("{bin} exited with status {}: {trimmed}", output.status)
        };
        return Err(msg);
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}
