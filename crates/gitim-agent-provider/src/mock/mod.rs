use async_trait::async_trait;
use std::time::Instant;
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use crate::{Event, ExecOptions, ExecResult, ExecStatus, Provider, ProviderConfig, ProviderError, ProviderUsage, Session};

/// MockProvider returns a fixed response without calling an LLM.
/// Used by E2E tests to exercise the full agent message loop.
pub struct MockProvider {
    #[allow(dead_code)]
    config: ProviderConfig,
    default_response: String,
    usage: Option<ProviderUsage>,
}

impl MockProvider {
    pub fn new(_config: ProviderConfig) -> Self {
        Self {
            config: _config,
            default_response: "mock-response: acknowledged".to_string(),
            usage: None,
        }
    }

    pub fn with_response(response: String) -> Self {
        Self {
            config: ProviderConfig::default(),
            default_response: response,
            usage: None,
        }
    }

    pub fn with_usage(mut self, usage: ProviderUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// Parse the channel name from a prompt of the form `[#channel] @author: message`.
    fn parse_channel(prompt: &str) -> Option<String> {
        let start = prompt.find("[#")? + 2;
        let end = prompt[start..].find(']')?;
        Some(prompt[start..start + end].to_string())
    }
}

#[async_trait]
impl Provider for MockProvider {
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError> {
        let channel = Self::parse_channel(prompt).unwrap_or_else(|| "general".to_string());
        let response = self.default_response.clone();
        let cwd = opts.cwd.clone();
        let usage = self.usage.clone();

        let (event_tx, event_rx) = mpsc::channel::<Event>(32);
        let (result_tx, result_rx) = oneshot::channel::<ExecResult>();

        let task = tokio::spawn(async move {
            let started = Instant::now();

            // Emit a Text event mirroring what the mock will send.
            let _ = event_tx
                .send(Event::Text {
                    content: response.clone(),
                })
                .await;

            // Emit ToolUse — running `gitim send`.
            let call_id = "mock-call-1".to_string();
            let _ = event_tx
                .send(Event::ToolUse {
                    tool: "bash".to_string(),
                    call_id: call_id.clone(),
                    input: serde_json::json!({
                        "command": format!("gitim send {} \"{}\"", channel, response)
                    }),
                })
                .await;

            // Run `gitim send <channel> "<response>"`.
            let mut cmd = std::process::Command::new("gitim");
            cmd.arg("send").arg(&channel).arg(&response);
            if let Some(dir) = &cwd {
                cmd.current_dir(dir);
            }
            let cmd_output = cmd.output();

            let tool_output = match cmd_output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    if out.status.success() {
                        stdout
                    } else {
                        format!("exit {}: {}", out.status, stderr)
                    }
                }
                Err(e) => format!("failed to run gitim: {e}"),
            };

            // Emit ToolResult.
            let _ = event_tx
                .send(Event::ToolResult {
                    call_id,
                    output: tool_output,
                })
                .await;

            let duration_ms = started.elapsed().as_millis() as u64;
            let _ = result_tx.send(ExecResult {
                status: ExecStatus::Completed,
                output: response,
                error: None,
                duration_ms,
                session_token: None,
                usage,
            });
        });

        Ok(Session::new(event_rx, result_rx, task.abort_handle(), CancellationToken::new()))
    }
}

#[cfg(test)]
mod usage_tests {
    use super::*;

    #[tokio::test]
    async fn mock_provider_emits_configured_usage() {
        let provider = MockProvider::with_response("ok".to_string())
            .with_usage(ProviderUsage {
                input_tokens: Some(42_000),
                output_tokens: Some(800),
                used_percent: None,
                ..Default::default()
            });

        let session = provider
            .execute("hi", ExecOptions::default())
            .await
            .expect("execute");

        // Drain events so the result channel fires.
        let mut events = session.events;
        while events.recv().await.is_some() {}

        let result = session.result.await.expect("result");
        assert_eq!(
            result.usage,
            Some(ProviderUsage {
                input_tokens: Some(42_000),
                output_tokens: Some(800),
                used_percent: None,
                ..Default::default()
            })
        );
    }

    #[tokio::test]
    async fn mock_provider_default_usage_is_none() {
        let provider = MockProvider::with_response("ok".to_string());
        let session = provider
            .execute("hi", ExecOptions::default())
            .await
            .expect("execute");

        let mut events = session.events;
        while events.recv().await.is_some() {}

        let result = session.result.await.expect("result");
        assert!(result.usage.is_none());
    }
}
