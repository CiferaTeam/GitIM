use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::RuntimeError;

/// A stateful wrapper around `claude -p` that tracks session_id for --resume.
pub struct ClaudeSession {
    session_id: Option<String>,
    system_prompt: String,
    allowed_tools: String,
    working_dir: PathBuf,
    model: Option<String>,
}

/// The result of a single claude -p invocation.
#[derive(Debug)]
pub struct ClaudeResult {
    pub text: String,
    pub session_id: String,
}

impl ClaudeSession {
    pub fn new(
        system_prompt: String,
        allowed_tools: &str,
        working_dir: &Path,
    ) -> Self {
        Self {
            session_id: None,
            system_prompt,
            allowed_tools: allowed_tools.to_string(),
            working_dir: working_dir.to_path_buf(),
            model: None,
        }
    }

    pub fn with_model(mut self, model: &str) -> Self {
        self.model = Some(model.to_string());
        self
    }

    /// Restore a previous session_id (for restart recovery).
    pub fn with_session_id(mut self, session_id: String) -> Self {
        self.session_id = Some(session_id);
        self
    }

    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Send a prompt to claude -p. First call creates a session, subsequent calls resume it.
    pub async fn send(&mut self, prompt: &str) -> Result<ClaudeResult, RuntimeError> {
        let mut args = vec![
            "-p".to_string(),
            prompt.to_string(),
            "--output-format".to_string(),
            "json".to_string(),
        ];

        if let Some(sid) = &self.session_id {
            args.push("--resume".to_string());
            args.push(sid.clone());
        } else {
            args.push("--system-prompt".to_string());
            args.push(self.system_prompt.clone());
        }

        if !self.allowed_tools.is_empty() {
            args.push("--allowedTools".to_string());
            args.push(self.allowed_tools.clone());
        }

        if let Some(model) = &self.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        let working_dir = self.working_dir.clone();
        let output = tokio::task::spawn_blocking(move || {
            Command::new("claude")
                .args(&args)
                .current_dir(&working_dir)
                .output()
        })
        .await
        .unwrap()
        .map_err(|e| RuntimeError::ClaudeFailed(format!("spawn failed: {e}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(RuntimeError::ClaudeFailed(format!(
                "exit code {}: {}",
                output.status.code().unwrap_or(-1),
                stderr
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout);

        // Output is a JSON array of messages: [{type, session_id, ...}, ...]
        let messages: Vec<serde_json::Value> = serde_json::from_str(stdout.trim())
            .map_err(|e| RuntimeError::ClaudeFailed(format!("parse output: {e}")))?;

        // Extract session_id from any message (typically the first "system/init" message)
        let session_id = messages
            .iter()
            .find_map(|m| m["session_id"].as_str())
            .ok_or_else(|| RuntimeError::ClaudeFailed("no session_id in output".into()))?
            .to_string();

        // Extract result text from the "result" type message
        let text = messages
            .iter()
            .filter(|m| m["type"].as_str() == Some("result"))
            .find_map(|m| m["result"].as_str())
            .unwrap_or("")
            .to_string();

        // Store session_id for future --resume calls
        self.session_id = Some(session_id.clone());

        Ok(ClaudeResult { text, session_id })
    }
}
