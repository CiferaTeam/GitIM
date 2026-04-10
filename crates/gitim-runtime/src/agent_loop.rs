use std::time::Duration;

use tracing::info;

use crate::claude::ClaudeSession;
use crate::error::RuntimeError;
use crate::poller::{ChannelChange, Poller};

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
}

impl AgentLoop {
    pub fn new(poller: Poller, claude: ClaudeSession, poll_interval: Duration) -> Self {
        Self {
            poller,
            claude,
            poll_interval,
        }
    }

    /// Build an AgentLoop with default system prompt and allowed tools.
    pub fn with_defaults(
        poller: Poller,
        working_dir: &std::path::Path,
    ) -> Self {
        let claude = ClaudeSession::new(
            DEFAULT_SYSTEM_PROMPT.to_string(),
            ALLOWED_TOOLS,
            working_dir,
        )
        .with_model("claude-sonnet-4-6");

        Self::new(poller, claude, Duration::from_secs(2))
    }

    /// Run one poll-and-process cycle. Returns true if messages were processed.
    pub async fn run_once(&mut self) -> Result<bool, RuntimeError> {
        let result = self.poller.poll().await?;

        if result.changes.is_empty() {
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

        Ok(true)
    }

    /// Run the agent loop indefinitely.
    pub async fn run(&mut self) -> Result<(), RuntimeError> {
        // First poll initializes cursor
        self.poller.poll().await?;
        info!("agent loop started, cursor initialized");

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
