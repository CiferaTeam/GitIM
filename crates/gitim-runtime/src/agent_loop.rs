use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_agent_provider::{ExecOptions, ExecStatus, Provider, ProviderConfig, create};
use gitim_client::GitimClient;
use tracing::info;

use crate::error::RuntimeError;
use crate::poller::{ChannelChange, Poller};
use crate::state::AgentState;

const DEFAULT_SYSTEM_PROMPT: &str = "\
你是一个 GitIM agent。你通过 GitIM 消息系统与其他参与者交流。

收到新消息后：
1. 理解消息内容
2. 如需回复，用以下命令发送：gitim send <channel> \"<回复内容>\"

注意：
- 直接使用 gitim send 命令回复，不要使用其他方式
- 每条回复独立调用一次 gitim send
";

pub struct AgentLoop {
    poller: Poller,
    provider: Box<dyn Provider>,
    session_token: Option<String>,
    poll_interval: Duration,
    repo_root: PathBuf,
    model: Option<String>,
}

impl AgentLoop {
    /// Build an AgentLoop with default settings. Restores state from disk if available.
    pub fn with_defaults(repo_root: &Path) -> Result<Self, RuntimeError> {
        let state = AgentState::load(repo_root)?;

        let poller = match state.cursor {
            Some(cursor) => {
                info!(cursor = %cursor, "restored cursor from state");
                Poller::with_cursor(GitimClient::new(repo_root), cursor)
            }
            None => Poller::new(GitimClient::new(repo_root)),
        };

        let provider = create("claude", ProviderConfig::default())
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

        if state.session_token.is_some() {
            info!("restored session_token from state");
        }

        Ok(Self {
            poller,
            provider,
            session_token: state.session_token,
            poll_interval: Duration::from_secs(2),
            repo_root: repo_root.to_path_buf(),
            model: Some("claude-sonnet-4-6".to_string()),
        })
    }

    fn save_state(&self) -> Result<(), RuntimeError> {
        let state = AgentState {
            cursor: self.poller.cursor().map(|s| s.to_string()),
            session_token: self.session_token.clone(),
        };
        state.save(&self.repo_root)
    }

    fn build_exec_options(&self) -> ExecOptions {
        ExecOptions {
            cwd: Some(self.repo_root.clone()),
            model: self.model.clone(),
            // Only pass system_prompt on first call; resume inherits it
            system_prompt: if self.session_token.is_none() {
                Some(DEFAULT_SYSTEM_PROMPT.to_string())
            } else {
                None
            },
            max_turns: Some(5),
            resume_token: self.session_token.clone(),
            ..Default::default()
        }
    }

    /// Run one poll-and-process cycle. Returns true if messages were processed.
    pub async fn run_once(&mut self) -> Result<bool, RuntimeError> {
        let result = self.poller.poll().await?;

        if result.changes.is_empty() {
            self.save_state()?;
            return Ok(false);
        }

        let prompt = format_changes_as_prompt(&result.changes);
        info!(prompt_len = prompt.len(), "sending to provider");

        let opts = self.build_exec_options();
        let session = self
            .provider
            .execute(&prompt, opts)
            .await
            .map_err(|e| RuntimeError::ProviderFailed(e.to_string()))?;

        // Drain events (log them)
        let mut events = session.events;
        while let Some(event) = events.recv().await {
            match &event {
                gitim_agent_provider::Event::Text { content } => {
                    tracing::debug!(text_len = content.len(), "agent text");
                }
                gitim_agent_provider::Event::ToolUse { tool, .. } => {
                    info!(tool = %tool, "agent tool use");
                }
                gitim_agent_provider::Event::Error { content } => {
                    tracing::warn!(error = %content, "agent error event");
                }
                _ => {}
            }
        }

        // Await final result
        let exec_result = session
            .result
            .await
            .map_err(|_| RuntimeError::ProviderFailed("result channel closed".into()))?;

        info!(
            status = ?exec_result.status,
            output_len = exec_result.output.len(),
            duration_ms = exec_result.duration_ms,
            "provider finished"
        );

        if exec_result.status == ExecStatus::Failed {
            tracing::error!(
                error = ?exec_result.error,
                "provider execution failed"
            );
            // Clear session_token to avoid resuming a broken session
            self.session_token = None;
        } else if let Some(token) = exec_result.session_token {
            self.session_token = Some(token);
        }

        self.save_state()?;
        Ok(true)
    }

    /// Run the agent loop indefinitely with exponential backoff on errors.
    pub async fn run(&mut self) -> Result<(), RuntimeError> {
        if self.poller.cursor().is_none() {
            self.poller.poll().await?;
            self.save_state()?;
            info!("agent loop started, cursor initialized");
        } else {
            info!("agent loop started, cursor restored from state");
        }

        let mut consecutive_errors: u32 = 0;
        const MAX_BACKOFF_SECS: u64 = 60;

        loop {
            match self.run_once().await {
                Ok(true) => {
                    consecutive_errors = 0;
                    info!("processed messages");
                }
                Ok(false) => {
                    consecutive_errors = 0;
                }
                Err(e) => {
                    consecutive_errors += 1;
                    let backoff = Duration::from_secs(
                        (2u64.saturating_pow(consecutive_errors)).min(MAX_BACKOFF_SECS),
                    );
                    tracing::error!(
                        error = %e,
                        consecutive = consecutive_errors,
                        backoff_secs = backoff.as_secs(),
                        "agent loop error, backing off"
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }
            }
            tokio::time::sleep(self.poll_interval).await;
        }
    }
}

fn format_changes_as_prompt(changes: &[ChannelChange]) -> String {
    let mut prompt = String::from("你收到了以下新消息：\n\n");

    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }

        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");
            let body = entry["body"].as_str().unwrap_or("");
            let channel = &change.channel;

            prompt.push_str(&format!("[#{channel}] @{author}: {body}\n"));
        }
    }

    prompt.push_str("\n请处理这些消息。");
    prompt
}
