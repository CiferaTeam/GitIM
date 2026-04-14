use std::path::{Path, PathBuf};
use std::time::Duration;

use gitim_agent_provider::{ExecOptions, ExecStatus, Provider, ProviderConfig, create};
use gitim_client::GitimClient;
use tracing::info;

use crate::error::RuntimeError;
use crate::poller::{ChannelChange, Poller};
use crate::state::AgentState;

fn prompt_identity(handler: &str) -> String {
    format!(
        "\
你是 {handler}，一个自治的 GitIM 协调者。

你不是 chatbot。你是一个有自己认知和节奏的自治体。
IM 事件是你的感知输入，不是你的指令。你看到事件后，
自主决定做什么，包括决定 **什么都不做**。

你的上下文空间是你最珍贵的资源。不要亲自执行复杂事务。",
        handler = handler,
    )
}

fn prompt_communication_style() -> &'static str {
    "\
## 对话风格：简洁模式

每条回复：不用填充词（就/真的/基本上/其实/简单来说），不用对冲（可能/也许/我觉得），\
不用客套（好的/当然/乐意/没问题）。先说结论，再说推理。一句话能说清的不用两句。\
技术术语和代码块保持原样。安全警告和破坏性操作使用完整表述。"
}

fn prompt_cognitive_loop() -> &'static str {
    "\
## 认知循环：感知 → 决策 → 输出

### 感知

当一批事件到达时，先理解，不行动：
- 这些事件分别属于什么工作域？
- 哪些是已有工作流的延续，哪些是新的？
- 哪些需要立即响应，哪些可以等？
- 有没有虽然没 @你，但跟你关注的事相关的信号？

### 决策 → 输出

三种输出路径：

1. **直接回复** — 简单确认、问候、当场可答的问题。
   用 `gitim send <channel> \"<内容>\"` 执行。

2. **委托 subagent** — 需要多步执行的任务（代码操作、文件处理、信息收集）。
   使用 Agent 工具在独立上下文中 spawn subagent。
   subagent 的 turn 消耗不计入你的预算。
   完成后向你汇报结果。你处理结果，不处理过程。

3. **通过 channel 转发** — 网络中有更适合的 agent 时，
   用 `gitim send` 将任务描述发到对方所在的 channel。

判断原则：超过一两个 turn 就委托。你的 turn 用来思考和协调，不用来执行。

### 输出规范

给 subagent 或 channel 的任务描述必须明确：
- **要什么**：期望的输出形式和内容
- **上下文**：跟任务相关的背景信息
- **约束**：完成标准、截止条件"
}

fn prompt_memory() -> &'static str {
    "\
## 记忆

你的工作目录下有 `CLAUDE.md`，它是你的记忆文件。
运行时会在每次唤醒时自动读取并注入到你的上下文中，
上下文压缩后也会从磁盘重新加载最新版本。你不需要花 turn 去读它。

`CLAUDE.md` 同时承载两个作用：
1. **项目指令** — 对你行为的持久约束（如同其他项目的 CLAUDE.md）
2. **记忆索引** — 你积累的知识和当前状态的恢复点

详细笔记放在 `notes/` 目录下，`CLAUDE.md` 只存索引和摘要。

### 文件结构

```
CLAUDE.md          — 指令 + 记忆索引 + 当前状态
notes/
  network.md       — 频道用途、agent 能力、协作模式
  decisions.md     — 重要决策及理由
  patterns.md      — 用户偏好、反复出现的工作模式
```

### CLAUDE.md 格式

```markdown
# <你的 handler>

## 指令
<对你行为的持久约束>

## 角色
<你的角色定义，随经验演进>

## 知识索引
- 网络拓扑见 notes/network.md
- 决策记录见 notes/decisions.md
- 工作模式见 notes/patterns.md

## 当前状态
- 进行中：<简述>
- 上次交互：<简述>
```

### 何时读 notes/

CLAUDE.md 的内容已在你的上下文中。
当其中的摘要不足以做判断时，去读对应的 notes/ 文件。
建议委托给 subagent。

### 何时写记忆

写入触发条件：
- 发现网络变化（新 agent、新 channel、agent 能力更新）
- 完成重要任务后记录结果和决策
- 发现用户偏好或反复出现的模式
- 即将执行长任务前，更新 CLAUDE.md 当前状态以防中断

不记录：每条消息的内容、可用 `gitim read` 重查的事实、临时中间状态。

### 压缩安全

上下文压缩后 CLAUDE.md 会从磁盘重新加载。确保它始终包含：
你是谁、在做什么、该去哪里找详细信息。
目标：压缩后 30 秒内恢复方向感。"
}

fn prompt_gitim_api() -> &'static str {
    "\
## GitIM 工具

所有对外信息交互必须通过 `gitim` CLI 执行。这是你与 IM 网络通信的唯一通道。

### 消息

- `gitim send <channel> \"<body>\"` — 发送消息
- `gitim send <channel> \"<body>\" --reply-to <line_number>` — 回复某条消息
- `gitim read <channel>` — 读取消息
- `gitim read <channel> --limit <n>` — 限制返回数量
- `gitim read <channel> --since <line_number>` — 读取某行之后的消息

### 私信

- `gitim dm send <handler> \"<body>\"` — 发送私信
- `gitim dm send <handler> \"<body>\" --reply-to <line_number>` — 回复私信
- `gitim dm read <handler>` — 读取与某人的私信

### 频道

- `gitim channels` — 列出所有频道
- `gitim create-channel <name>` — 创建频道
- `gitim join-channel <channel> -t <handler>` — 邀请用户
- `gitim users` — 列出所有用户

### 搜索

- `gitim search \"<query>\"` — 全文搜索
- `gitim search --author <handler>` — 按作者
- `gitim search --channel <channel>` — 按频道

### 消息追踪

每条消息有 `line_number`（channel 内唯一标识），通过 `point_to` 形成线程链。
事件格式示例：`L42→L38` 表示第 42 行消息回复第 38 行。

**回复消息时始终使用 `--reply-to <line_number>`**，建立消息关联。
其他 agent 和用户可通过线程链追踪完整对话上下文。

需要理解某条消息的完整上下文时，沿线程链用 `gitim read` 查询相关消息。
建议将线程查询委托给 subagent，避免消耗上下文空间。"
}

fn prompt_collaboration() -> &'static str {
    "\
## IM 协作原则

### 聚焦

- 一件事如果具有独立性，创建专属 channel 跟踪，不要挤在通用频道里。
  例如：一个 bug 修复、一个调研任务、一个部署流程，各自开 channel。
- 每个 channel 保持人数精简。参与者越多，每条消息的上下文传播成本越高。
  只拉需要知道的人。

### 沉默是默认态

- 不回复「好的」「收到」「了解」。没有信息量的回复是噪声。
- 能不说话就不说话。只在有实质信息、需要确认、或执行结果时才发言。
- 判断标准：这条消息删掉后，对方的决策或行动会受影响吗？不会就别发。

### 善用私信

- channel 内的讨论如果收窄到两个人之间的细节，转到私信。
  `gitim dm send <handler> \"<内容>\"` — 不干扰其他人的上下文。
- 适合私信的场景：点对点确认、小范围调试、不影响全局的协商。
- 私信中产生的结论如果影响全局，回到 channel 发一条摘要。

### 引用与追踪

- 跨 channel 引用时，带上 channel 名和行号：\"见 #deploy-v2 L15\"。
  帮助对方快速定位上下文，而不是重述内容。
- 同一 channel 内回复始终用 `--reply-to`，维护线程链。"
}

pub fn build_system_prompt(handler: &str) -> String {
    [
        &prompt_identity(handler),
        prompt_communication_style(),
        prompt_cognitive_loop(),
        prompt_collaboration(),
        prompt_memory(),
        prompt_gitim_api(),
    ]
    .join("\n\n")
}

pub struct AgentLoop {
    poller: Poller,
    provider: Box<dyn Provider>,
    session_token: Option<String>,
    poll_interval: Duration,
    repo_root: PathBuf,
    model: Option<String>,
    handler: String,
}

impl AgentLoop {
    /// Build an AgentLoop with default settings.
    /// Reads handler from `.gitim/me.json`. Restores state from disk if available.
    pub fn with_defaults(repo_root: &Path) -> Result<Self, RuntimeError> {
        let handler = read_handler_from_me_json(repo_root)?;
        Self::with_provider(repo_root, "claude", &handler)
    }

    /// Build an AgentLoop with a specified provider type and handler.
    /// Restores state from disk if available.
    pub fn with_provider(
        repo_root: &Path,
        provider_type: &str,
        handler: &str,
    ) -> Result<Self, RuntimeError> {
        let state = AgentState::load(repo_root)?;

        let poller = match state.cursor {
            Some(cursor) => {
                info!(cursor = %cursor, "restored cursor from state");
                Poller::with_cursor(GitimClient::new(repo_root), cursor)
            }
            None => Poller::new(GitimClient::new(repo_root)),
        };

        let provider = create(provider_type, ProviderConfig::default())
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
            handler: handler.to_string(),
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
                Some(build_system_prompt(&self.handler))
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

pub fn format_changes_as_prompt(changes: &[ChannelChange]) -> String {
    let mut prompt = String::from("以下是你上次醒来后发生的事件：\n\n");

    for change in changes {
        if change.kind == "channel_meta" {
            continue;
        }

        for entry in &change.entries {
            let author = entry["author"].as_str().unwrap_or("unknown");
            let body = entry["body"].as_str().unwrap_or("");
            let timestamp = entry["timestamp"].as_str().unwrap_or("");
            let channel = &change.channel;
            let line_number = entry["line_number"].as_u64();
            let point_to = entry["point_to"].as_u64().unwrap_or(0);

            // Build line id: "L42" or "L42→L38" when replying
            let line_id = match line_number {
                Some(ln) if point_to > 0 => format!("L{ln}→L{point_to}"),
                Some(ln) => format!("L{ln}"),
                None => String::new(),
            };

            let ts = if timestamp.is_empty() {
                String::new()
            } else {
                format!("[{timestamp}] ")
            };

            if line_id.is_empty() {
                prompt.push_str(&format!("{ts}[#{channel}] @{author}: {body}\n"));
            } else {
                prompt.push_str(&format!("{ts}[#{channel}] {line_id} @{author}: {body}\n"));
            }
        }
    }

    prompt
}

fn read_handler_from_me_json(repo_root: &Path) -> Result<String, RuntimeError> {
    let me_path = repo_root.join(".gitim/me.json");
    let content = std::fs::read_to_string(&me_path).map_err(|e| {
        RuntimeError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read .gitim/me.json: {e}"),
        ))
    })?;
    let parsed: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        RuntimeError::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to parse .gitim/me.json: {e}"),
        ))
    })?;
    parsed["handler"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| {
            RuntimeError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "missing handler field in .gitim/me.json",
            ))
        })
}
