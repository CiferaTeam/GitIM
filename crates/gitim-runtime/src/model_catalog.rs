use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const DEFAULT_BIN_CODEX: &str = "codex";
const DEFAULT_BIN_OPENCODE: &str = "opencode";
const DEFAULT_BIN_PI: &str = "pi";
const DEFAULT_BIN_CURSOR: &str = "cursor-agent";
const DEFAULT_BIN_KIMI: &str = "kimi";
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
    pub pi_bin: Option<String>,
    pub cursor_bin: Option<String>,
    pub kimi_bin: Option<String>,
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
        "pi" => {
            PiCatalog {
                bin: overrides
                    .pi_bin
                    .unwrap_or_else(|| DEFAULT_BIN_PI.to_string()),
            }
            .list_models()
            .await
        }
        "cursor" => {
            CursorCatalog {
                bin: overrides
                    .cursor_bin
                    .unwrap_or_else(|| DEFAULT_BIN_CURSOR.to_string()),
            }
            .list_models()
            .await
        }
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
        "kimi" => {
            KimiCatalog {
                bin: overrides
                    .kimi_bin
                    .unwrap_or_else(|| DEFAULT_BIN_KIMI.to_string()),
            }
            .list_models()
            .await
        }
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

struct PiCatalog {
    bin: String,
}

#[async_trait]
impl ModelCatalogProvider for PiCatalog {
    async fn list_models(&self) -> ModelCatalogResult {
        match run_catalog_command_output(&self.bin, &["--list-models"]).await {
            Ok(output) => {
                let combined = format!("{}\n{}", output.stdout, output.stderr);
                fallback_catalog(
                    "pi",
                    "pi_list_models",
                    true,
                    true,
                    Some("provider/model or model".to_string()),
                    None,
                )
                .with_models(parse_pi_models(&combined))
            }
            Err(error) => fallback_catalog(
                "pi",
                "pi_list_models",
                true,
                true,
                Some("provider/model or model".to_string()),
                Some(error),
            ),
        }
    }
}

struct CursorCatalog {
    bin: String,
}

#[async_trait]
impl ModelCatalogProvider for CursorCatalog {
    async fn list_models(&self) -> ModelCatalogResult {
        match run_catalog_command(&self.bin, &["models"]).await {
            Ok(stdout) => fallback_catalog(
                "cursor",
                "cursor_models",
                true,
                true,
                Some("model id accepted by cursor-agent --model".to_string()),
                None,
            )
            .with_models(parse_cursor_models(&stdout)),
            Err(error) => fallback_catalog(
                "cursor",
                "cursor_models",
                true,
                true,
                Some("model id accepted by cursor-agent --model".to_string()),
                Some(error),
            ),
        }
    }
}

struct KimiCatalog {
    bin: String,
}

#[async_trait]
impl ModelCatalogProvider for KimiCatalog {
    async fn list_models(&self) -> ModelCatalogResult {
        match run_kimi_acp_model_catalog(&self.bin).await {
            Ok(models) => fallback_catalog(
                "kimi",
                "kimi_acp_models",
                true,
                true,
                Some("model id accepted by kimi set_session_model".to_string()),
                None,
            )
            .with_models(models),
            Err(error) => fallback_catalog(
                "kimi",
                "kimi_acp_models",
                true,
                true,
                Some("model id accepted by kimi set_session_model".to_string()),
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

pub fn parse_pi_models(stdout: &str) -> Vec<ModelOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("No models") {
            continue;
        }

        let columns: Vec<_> = line.split_whitespace().collect();
        if columns.len() < 6 {
            continue;
        }

        let provider = columns[0];
        let model = columns[1];
        if provider == "provider" && model == "model" {
            continue;
        }

        let id = format!("{provider}/{model}");
        if !seen.insert(id.clone()) {
            continue;
        }

        out.push(ModelOption {
            label: id.clone(),
            id,
        });
    }

    out
}

pub fn parse_cursor_models(stdout: &str) -> Vec<ModelOption> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty()
            || line == "Available models"
            || line.starts_with("Tip:")
            || line.starts_with("No models")
        {
            continue;
        }

        let Some((id, label)) = line.split_once(" - ") else {
            continue;
        };
        let id = id.trim();
        let label = label.trim();
        if id.is_empty() || !seen.insert(id.to_string()) {
            continue;
        }

        out.push(ModelOption {
            id: id.to_string(),
            label: label.to_string(),
        });
    }

    out
}

pub fn parse_kimi_session_models(result: &Value) -> Vec<ModelOption> {
    let Some(models) = result
        .get("models")
        .and_then(|m| m.get("availableModels"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    let current = result
        .get("models")
        .and_then(|m| m.get("currentModelId"))
        .and_then(Value::as_str);
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for model in models {
        let Some(id) = model.get("modelId").and_then(Value::as_str) else {
            continue;
        };
        if id.trim().is_empty() || !seen.insert(id.to_string()) {
            continue;
        }
        let name = model
            .get("name")
            .and_then(Value::as_str)
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(id);
        let label = if current == Some(id) && !name.to_lowercase().contains("default") {
            format!("{name} (default)")
        } else {
            name.to_string()
        };
        out.push(ModelOption {
            id: id.to_string(),
            label,
        });
    }
    out
}

async fn run_kimi_acp_model_catalog(bin: &str) -> Result<Vec<ModelOption>, String> {
    let temp_dir = std::env::temp_dir().join(format!(
        "gitim-kimi-models-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
    ));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("failed to create kimi model temp dir: {e}"))?;

    let mut child = tokio::process::Command::new(bin);
    child
        .args(["--afk", "acp"])
        .current_dir(&temp_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match child.spawn() {
        Ok(child) => child,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&temp_dir);
            return Err(format!("failed to spawn {bin}: {e}"));
        }
    };

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| format!("{bin} stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| format!("{bin} stdout unavailable"))?;
    let stderr = child.stderr.take();
    let mut lines = BufReader::new(stdout).lines();

    let result = async {
        let _init = kimi_acp_request(
            &mut stdin,
            &mut lines,
            0,
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {"name": "gitim-runtime", "version": "0.1.0"},
                "clientCapabilities": {},
            }),
        )
        .await?;
        let session = kimi_acp_request(
            &mut stdin,
            &mut lines,
            1,
            "session/new",
            json!({ "cwd": temp_dir.to_string_lossy(), "mcpServers": [] }),
        )
        .await?;
        Ok::<Vec<ModelOption>, String>(parse_kimi_session_models(&session))
    }
    .await;

    let _ = child.start_kill();
    let _ = child.wait().await;
    let _ = std::fs::remove_dir_all(&temp_dir);

    match result {
        Ok(models) if models.is_empty() => {
            let stderr_text = if let Some(stderr) = stderr {
                read_stderr_tail(stderr).await
            } else {
                String::new()
            };
            if stderr_text.trim().is_empty() {
                Err("kimi ACP session/new returned no models".to_string())
            } else {
                Err(format!(
                    "kimi ACP session/new returned no models: {}",
                    stderr_text.trim()
                ))
            }
        }
        other => other,
    }
}

async fn kimi_acp_request(
    stdin: &mut tokio::process::ChildStdin,
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: i64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    });
    let mut payload =
        serde_json::to_vec(&request).map_err(|e| format!("serialize {method}: {e}"))?;
    payload.push(b'\n');
    stdin
        .write_all(&payload)
        .await
        .map_err(|e| format!("write {method}: {e}"))?;
    let response = tokio::time::timeout(CATALOG_TIMEOUT, async {
        loop {
            let Some(line) = lines
                .next_line()
                .await
                .map_err(|e| format!("read {method}: {e}"))?
            else {
                return Err(format!("{method}: kimi stdout ended"));
            };
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if v.get("id").and_then(Value::as_i64) != Some(id) {
                continue;
            }
            if let Some(error) = v.get("error") {
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                return Err(format!("{method}: {message}"));
            }
            return Ok(v.get("result").cloned().unwrap_or(Value::Null));
        }
    })
    .await
    .map_err(|_| {
        format!(
            "{method}: kimi ACP request exceeded {}ms",
            CATALOG_TIMEOUT.as_millis()
        )
    })??;
    Ok(response)
}

async fn read_stderr_tail(stderr: tokio::process::ChildStderr) -> String {
    let mut lines = BufReader::new(stderr).lines();
    let mut out = Vec::new();
    while let Ok(Some(line)) = lines.next_line().await {
        out.push(line);
        if out.len() > 20 {
            out.remove(0);
        }
    }
    out.join("\n")
}

async fn run_catalog_command(bin: &str, args: &[&str]) -> Result<String, String> {
    run_catalog_command_output(bin, args)
        .await
        .map(|output| output.stdout)
}

struct CatalogCommandOutput {
    stdout: String,
    stderr: String,
}

async fn run_catalog_command_output(
    bin: &str,
    args: &[&str],
) -> Result<CatalogCommandOutput, String> {
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

    Ok(CatalogCommandOutput {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}
