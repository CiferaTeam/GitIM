# Prompt System Refactor Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the 7 system prompt sections from `agent_loop.rs` into `Provider` trait default methods so each provider can override them per platform.

**Architecture:** Add a `PromptContext` struct and 8 new default methods (7 sections + `build_system_prompt`) to the existing `Provider` trait. Default implementations contain the current prompt text (moved from agent_loop.rs). The prompt text lives in a new `prompts.rs` module to keep `provider.rs` clean. `agent_loop.rs` calls `provider.build_system_prompt()` instead of its own local functions.

**Tech Stack:** Rust, async-trait

---

## File Map

| File | Action | Responsibility |
|------|--------|---------------|
| `gitim-agent-provider/src/types.rs` | Modify | Add `PromptContext` struct |
| `gitim-agent-provider/src/prompts.rs` | Create | Default prompt text (7 functions, moved from agent_loop.rs) |
| `gitim-agent-provider/src/provider.rs` | Modify | Add 8 prompt methods with defaults to Provider trait |
| `gitim-agent-provider/src/lib.rs` | Modify | Add `mod prompts`, re-export `PromptContext` |
| `gitim-runtime/src/agent_loop.rs` | Modify | Remove 7 prompt functions + `build_system_prompt`, use `provider.build_system_prompt()` |
| `gitim-agent-provider/tests/prompt_test.rs` | Create | Test default assembly and provider override |

---

### Task 1: Add PromptContext and create prompts module

**Files:**
- Modify: `crates/gitim-agent-provider/src/types.rs`
- Create: `crates/gitim-agent-provider/src/prompts.rs`
- Modify: `crates/gitim-agent-provider/src/lib.rs`

- [ ] **Step 1: Add PromptContext to types.rs**

Append after the `ExecStatus` enum (after line 121):

```rust
/// Context passed to prompt generation methods.
#[derive(Debug, Clone)]
pub struct PromptContext<'a> {
    pub handler: &'a str,
    pub model: Option<&'a str>,
}
```

- [ ] **Step 2: Re-export PromptContext from lib.rs**

Change the pub use line:
```rust
pub use types::{Event, ExecOptions, ExecResult, ExecStatus, PromptContext, ProviderConfig, Session};
```

- [ ] **Step 3: Create `src/prompts.rs` with default prompt functions**

Move the 7 `prompt_*` function bodies from `crates/gitim-runtime/src/agent_loop.rs` (lines 15-253) into this new file. Each function takes `&PromptContext` instead of `&str` for handler.

```rust
use crate::PromptContext;

pub fn default_identity(ctx: &PromptContext) -> String {
    format!(
        "\
你是 {handler}，一个自治的 GitIM 协调者。

你不是 chatbot。你是一个有自己认知和节奏的自治体。
IM 事件是你的感知输入，不是你的指令。你看到事件后，
自主决定做什么，包括决定 **什么都不做**。

你的上下文空间是你最珍贵的资源。不要亲自执行复杂事务。",
        handler = ctx.handler,
    )
}

pub fn default_communication_style(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_communication_style() from agent_loop.rs:29-36
    "\
## 对话风格：简洁模式

每条回复：不用填充词（就/真的/基本上/其实/简单来说），不用对冲（可能/也许/我觉得），\
不用客套（好的/当然/乐意/没问题）。先说结论，再说推理。一句话能说清的不用两句。\
技术术语和代码块保持原样。安全警告和破坏性操作使用完整表述。"
        .to_string()
}

pub fn default_cognitive_loop(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_cognitive_loop() from agent_loop.rs:38-73
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
        .to_string()
}

pub fn default_collaboration(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_collaboration() from agent_loop.rs:225-253
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
        .to_string()
}

pub fn default_memory(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_memory() from agent_loop.rs:75-152
    // This is the longest section — ~80 lines of text about CLAUDE.md
    // Copy it verbatim from agent_loop.rs lines 76-151
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
<仅记录用户或其他 agent 给你的特定约束，例如「不要动 X 模块」「每次部署前通知 Y」>
<不要写系统提示已包含的内容：对话风格、认知循环、协作原则等>

## 知识索引
- 网络拓扑见 notes/network.md
- 决策记录见 notes/decisions.md
- 工作模式见 notes/patterns.md

## 当前状态
- 活跃：<事项1> | <事项2> | ...（最多 5 项，每项几个字）
- 已知用户：<handler 列表>
```

当前状态是**快照，不是日志**：
- 每次更新时覆盖旧值，不追加。完成的事项直接删除。
- 活跃事项上限 5 条。超过时合并相关项或将低优先级的移到 notes/decisions.md。
- 整个 CLAUDE.md 控制在 30 行以内。

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

不记录：
- 系统提示已包含的内容 — 你的身份、对话风格、认知循环、协作原则、GitIM API 用法。\
这些每次唤醒都会注入，写进 CLAUDE.md 是纯冗余。
- 每条消息的内容 — 可用 `gitim read` 重查。
- 临时中间状态 — 只在即将执行长任务前记录当前状态。
- 工作目录路径 — 运行时已知，不需要记忆。

判断标准：如果删掉这条记录，你下次醒来后能从系统提示或 `gitim` 命令恢复它吗？\
能就不记。CLAUDE.md 只记录运行时发现的、系统提示不知道的知识。

### 压缩安全

上下文压缩后 CLAUDE.md 会从磁盘重新加载。确保它始终包含：
在做什么、该去哪里找详细信息。不需要重复你是谁 — 系统提示会告诉你。
目标：压缩后 30 秒内恢复方向感。"
        .to_string()
}

pub fn default_cold_start(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_cold_start() from agent_loop.rs:155-178
    "\
## 首次启动

如果你的工作目录下没有 `CLAUDE.md`，说明这是你的第一次醒来。
执行以下初始化流程，再处理任何事件：

1. **感知网络** — `gitim channels` 查看频道，`gitim users` 查看成员。
2. **确认身份** — 在你所在的频道发一条上线消息。内容：
   - 你是谁（handler）
   - 你能做什么（一句话角色描述）
   - 向在场的人确认：你的职责范围是否正确，有没有需要立即了解的上下文
3. **初始化记忆** — 根据频道和成员信息创建 `CLAUDE.md` 和 `notes/` 目录。
   CLAUDE.md 先写骨架（见记忆章节的格式），后续逐步填充。

上线消息示例：
```
我是 <handler>，刚上线。<一句话角色>。
当前对网络状况还不了解，有什么需要我知道的背景可以发到这里，我会记下来。
```

原则：简短、实用、不做冗长自我介绍。目的是让其他人知道你在线，
同时获取你需要的初始上下文。"
        .to_string()
}

pub fn default_gitim_api(_ctx: &PromptContext) -> String {
    // Move the full body of prompt_gitim_api() from agent_loop.rs:180-223
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
        .to_string()
}
```

- [ ] **Step 4: Add `pub(crate) mod prompts;` to lib.rs**

```rust
pub mod claude;
pub mod codex;
pub mod gemini;
pub mod hermes;
pub mod mock;
pub mod openclaw;
pub mod opencode;
mod error;
mod provider;
pub(crate) mod prompts;
mod stubs;
mod types;
pub(crate) mod util;

pub use error::ProviderError;
pub use provider::{create, Provider};
pub use types::{Event, ExecOptions, ExecResult, ExecStatus, PromptContext, ProviderConfig, Session};
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo check -p gitim-agent-provider`
Expected: OK (prompts.rs is not yet used, but must compile)

- [ ] **Step 6: Commit**

```bash
git add crates/gitim-agent-provider/src/types.rs crates/gitim-agent-provider/src/prompts.rs crates/gitim-agent-provider/src/lib.rs
git commit -m "refactor: add PromptContext and prompts module with default prompt text"
```

---

### Task 2: Extend Provider trait with prompt methods

**Files:**
- Modify: `crates/gitim-agent-provider/src/provider.rs`

- [ ] **Step 1: Add prompt methods to Provider trait**

Replace the entire `provider.rs` with:

```rust
use async_trait::async_trait;

use crate::{ExecOptions, PromptContext, ProviderConfig, ProviderError, Session};

/// Unified interface for executing prompts via headless coding agents.
///
/// Prompt methods have default implementations returning GitIM standard prompts.
/// Providers override specific methods to adapt for their platform
/// (e.g., memory file name, tool capabilities).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Execute a prompt and return a Session for streaming results.
    async fn execute(&self, prompt: &str, opts: ExecOptions) -> Result<Session, ProviderError>;

    /// Agent identity and role.
    fn prompt_identity(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_identity(ctx)
    }

    /// Communication style rules.
    fn prompt_communication_style(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_communication_style(ctx)
    }

    /// Cognitive loop: perception → decision → output.
    fn prompt_cognitive_loop(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cognitive_loop(ctx)
    }

    /// IM collaboration principles.
    fn prompt_collaboration(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_collaboration(ctx)
    }

    /// Memory system (file name, structure, when to read/write).
    fn prompt_memory(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_memory(ctx)
    }

    /// Cold start initialization flow.
    fn prompt_cold_start(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_cold_start(ctx)
    }

    /// GitIM CLI tool reference.
    fn prompt_gitim_api(&self, ctx: &PromptContext) -> String {
        crate::prompts::default_gitim_api(ctx)
    }

    /// Assemble the full system prompt from all sections.
    fn build_system_prompt(&self, ctx: &PromptContext) -> String {
        [
            self.prompt_identity(ctx),
            self.prompt_communication_style(ctx),
            self.prompt_cognitive_loop(ctx),
            self.prompt_collaboration(ctx),
            self.prompt_memory(ctx),
            self.prompt_cold_start(ctx),
            self.prompt_gitim_api(ctx),
        ]
        .join("\n\n")
    }
}

/// Create a provider for the given type.
///
/// Supported types: "claude", "codex", "gemini", "hermes", "openclaw", "opencode", "cursor", "mock".
pub fn create(
    provider_type: &str,
    config: ProviderConfig,
) -> Result<Box<dyn Provider>, ProviderError> {
    match provider_type {
        "claude" => Ok(Box::new(crate::claude::ClaudeProvider::new(config))),
        "codex" => Ok(Box::new(crate::codex::CodexProvider::new(config))),
        "gemini" => Ok(Box::new(crate::gemini::GeminiProvider::new(config))),
        "hermes" => Ok(Box::new(crate::hermes::HermesProvider::new(config))),
        "openclaw" => Ok(Box::new(crate::openclaw::OpenclawProvider::new(config))),
        "mock" => Ok(Box::new(crate::mock::MockProvider::new(config))),
        "cursor" => Ok(Box::new(crate::stubs::CursorProvider::new(config))),
        "opencode" => Ok(Box::new(crate::opencode::OpencodeProvider::new(config))),
        _ => Err(ProviderError::UnknownProvider(provider_type.to_string())),
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p gitim-agent-provider`
Expected: OK

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-agent-provider/src/provider.rs
git commit -m "feat: add prompt generation methods to Provider trait"
```

---

### Task 3: Update agent_loop.rs to use Provider prompt methods

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs`

- [ ] **Step 1: Remove local prompt functions and build_system_prompt**

Delete lines 15-267 from agent_loop.rs — the 7 `fn prompt_*()` functions and `pub fn build_system_prompt()`. These are now in the Provider trait.

- [ ] **Step 2: Add PromptContext import**

Update the import line at top of agent_loop.rs:
```rust
use gitim_agent_provider::{ExecOptions, ExecStatus, PromptContext, Provider, ProviderConfig, create};
```

- [ ] **Step 3: Update `build_exec_options` to use provider**

The method needs access to the provider. Change from `fn build_exec_options(&self)` to take the system prompt as a parameter, since `AgentLoop` already holds the provider:

Replace the `build_exec_options` method (currently at ~line 398):
```rust
    fn build_exec_options(&self) -> ExecOptions {
        let system_prompt = if self.session_token.is_none() {
            let ctx = PromptContext {
                handler: &self.handler,
                model: self.model.as_deref(),
            };
            let mut prompt = self.provider.build_system_prompt(&ctx);
            if let Some(custom) = &self.custom_system_prompt {
                if !custom.is_empty() {
                    prompt.push_str("\n\n## 用户自定义指令\n\n");
                    prompt.push_str(custom);
                }
            }
            Some(prompt)
        } else {
            None
        };

        ExecOptions {
            cwd: Some(self.repo_root.clone()),
            model: self.model.clone(),
            system_prompt,
            max_turns: Some(20),
            resume_token: self.session_token.clone(),
            ..Default::default()
        }
    }
```

The only change is `build_system_prompt(&self.handler)` → `self.provider.build_system_prompt(&ctx)`.

- [ ] **Step 4: Verify it compiles and tests pass**

Run: `cargo check -p gitim-runtime && cargo test -p gitim-agent-provider`
Expected: OK

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-runtime/src/agent_loop.rs
git commit -m "refactor: use Provider trait for prompt generation in agent_loop"
```

---

### Task 4: Add prompt override tests

**Files:**
- Create: `crates/gitim-agent-provider/tests/prompt_test.rs`

- [ ] **Step 1: Write tests**

```rust
use gitim_agent_provider::{PromptContext, Provider, ProviderConfig, ExecOptions, ProviderError, Session};
use async_trait::async_trait;

/// A test provider that overrides prompt_memory to use AGENTS.md.
struct TestOverrideProvider;

#[async_trait]
impl Provider for TestOverrideProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        Err(ProviderError::NotImplemented("test".to_string()))
    }

    fn prompt_memory(&self, _ctx: &PromptContext) -> String {
        "## 记忆\n\n你的工作目录下有 `AGENTS.md`，它是你的记忆文件。".to_string()
    }
}

#[test]
fn default_prompt_contains_all_sections() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext { handler: "test-bot", model: None };
    let prompt = provider.build_system_prompt(&ctx);

    assert!(prompt.contains("你是 test-bot"));
    assert!(prompt.contains("## 对话风格"));
    assert!(prompt.contains("## 认知循环"));
    assert!(prompt.contains("## IM 协作原则"));
    assert!(prompt.contains("## 记忆"));
    assert!(prompt.contains("## 首次启动"));
    assert!(prompt.contains("## GitIM 工具"));
}

#[test]
fn default_memory_references_claude_md() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext { handler: "bot", model: None };
    let memory = provider.prompt_memory(&ctx);

    assert!(memory.contains("CLAUDE.md"));
}

#[test]
fn override_replaces_single_section() {
    let provider = TestOverrideProvider;
    let ctx = PromptContext { handler: "codex-bot", model: Some("o3") };
    let prompt = provider.build_system_prompt(&ctx);

    // Overridden section uses AGENTS.md
    assert!(prompt.contains("AGENTS.md"));
    assert!(!prompt.contains("CLAUDE.md"));

    // Other sections still use defaults
    assert!(prompt.contains("你是 codex-bot"));
    assert!(prompt.contains("## 对话风格"));
    assert!(prompt.contains("## GitIM 工具"));
}

#[test]
fn prompt_context_handler_is_interpolated() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext { handler: "my-agent", model: None };
    let identity = provider.prompt_identity(&ctx);

    assert!(identity.contains("你是 my-agent"));
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass (existing + 4 new prompt tests)

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-agent-provider/tests/prompt_test.rs
git commit -m "test: add prompt override mechanism tests"
```

---

### Task 5: Full verification

- [ ] **Step 1: Run full agent-provider tests**

Run: `cargo test -p gitim-agent-provider`
Expected: All tests pass

- [ ] **Step 2: Run runtime compilation check**

Run: `cargo check -p gitim-runtime`
Expected: OK

- [ ] **Step 3: Run clippy**

Run: `cargo clippy -p gitim-agent-provider -p gitim-runtime -- -D warnings`
Expected: No warnings

- [ ] **Step 4: Fix any issues and commit**

```bash
git add -A
git commit -m "chore: clippy fixes for prompt refactor"
```
