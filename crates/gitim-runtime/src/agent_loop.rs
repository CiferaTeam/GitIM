use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_client::GitimClient;
use tracing::info;

use crate::claude::ClaudeSession;
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

const ALLOWED_TOOLS: &str = "Bash(gitim *),Read";

pub struct AgentLoop {
    poller: Poller,
    claude: ClaudeSession,
    poll_interval: Duration,
    repo_root: PathBuf,
}

impl AgentLoop {
    pub fn new(
        poller: Poller,
        claude: ClaudeSession,
        poll_interval: Duration,
        repo_root: &Path,
    ) -> Self {
        Self {
            poller,
            claude,
            poll_interval,
            repo_root: repo_root.to_path_buf(),
        }
    }

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

        let mut claude = ClaudeSession::new(
            DEFAULT_SYSTEM_PROMPT.to_string(),
            ALLOWED_TOOLS,
            repo_root,
        )
        .with_model("claude-sonnet-4-6");

        if let Some(session_id) = state.session_id {
            info!(session_id = %session_id, "restored session from state");
            claude = claude.with_session_id(session_id);
        }

        Ok(Self::new(poller, claude, Duration::from_secs(2), repo_root))
    }

    /// Save current state to disk.
    fn save_state(&self) -> Result<(), RuntimeError> {
        let state = AgentState {
            cursor: self.poller.cursor().map(|s| s.to_string()),
            session_id: self.claude.session_id().map(|s| s.to_string()),
        };
        state.save(&self.repo_root)
    }

    /// Run one poll-and-process cycle. Returns true if messages were processed.
    pub async fn run_once(&mut self) -> Result<bool, RuntimeError> {
        let result = self.poller.poll().await?;

        if result.changes.is_empty() {
            // Save cursor even when no messages (init case)
            self.save_state()?;
            return Ok(false);
        }

        let prompt = format_changes_as_prompt(&result.changes);
        info!(prompt_len = prompt.len(), "sending to claude");

        let response = self.claude.send(&prompt).await?;
        info!(
            response_len = response.text.len(),
            session_id = %response.session_id,
            "claude responded"
        );

        self.save_state()?;
        Ok(true)
    }

    /// Run the agent loop indefinitely.
    pub async fn run(&mut self) -> Result<(), RuntimeError> {
        // Initialize cursor if not restored from state
        if self.poller.cursor().is_none() {
            self.poller.poll().await?;
            self.save_state()?;
            info!("agent loop started, cursor initialized");
        } else {
            info!("agent loop started, cursor restored from state");
        }

        loop {
            match self.run_once().await {
                Ok(true) => info!("processed messages"),
                Ok(false) => {}
                Err(e) => {
                    tracing::error!(error = %e, "agent loop error");
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
