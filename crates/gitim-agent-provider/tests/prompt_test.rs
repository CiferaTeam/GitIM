use async_trait::async_trait;
use gitim_agent_provider::{
    ExecOptions, PromptContext, Provider, ProviderConfig, ProviderError, Session,
};

/// A test provider that overrides prompt_memory with a minimal stub.
struct TestOverrideProvider;

#[async_trait]
impl Provider for TestOverrideProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        Err(ProviderError::NotImplemented("test".to_string()))
    }

    fn prompt_memory(&self, _ctx: &PromptContext) -> String {
        "## 记忆\n\n这是被 override 的最小记忆段。".to_string()
    }
}

/// A test provider that suppresses the cron usage section. Models a
/// hypothetical future provider whose execution surface doesn't include
/// shell tool access (so teaching it cron syntax would be misleading).
struct CronlessProvider;

#[async_trait]
impl Provider for CronlessProvider {
    async fn execute(&self, _prompt: &str, _opts: ExecOptions) -> Result<Session, ProviderError> {
        Err(ProviderError::NotImplemented("test".to_string()))
    }

    fn prompt_cron_usage(&self, _ctx: &PromptContext) -> String {
        String::new()
    }
}

#[test]
fn default_prompt_contains_all_sections() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "test-bot",
        model: None,
    };
    let prompt = provider.build_system_prompt(&ctx);

    assert!(prompt.contains("你是 test-bot"));
    assert!(prompt.contains("## 对话风格"));
    assert!(prompt.contains("## 认知循环"));
    assert!(prompt.contains("## IM 协作原则"));
    assert!(prompt.contains("## 记忆"));
    assert!(prompt.contains("## 主动净化上下文"));
    assert!(prompt.contains("[[RESET]]"));
    assert!(prompt.contains("leave-channel"));
    assert!(prompt.contains("## 首次启动"));
    assert!(prompt.contains("## GitIM 工具"));
    assert!(prompt.contains("## 主机操作边界"));
    assert!(prompt.contains("pkill -f gitim-daemon"));
    // Cron usage is part of the default surface — assert presence and
    // canonical command syntax. The substring "gitim cron create" is
    // narrow enough that wording tweaks elsewhere in the section won't
    // flap it, but specific enough to fail loudly if Lane B ever renames
    // the subcommand.
    assert!(prompt.contains("## 周期任务"));
    assert!(prompt.contains("gitim cron create"));
}

#[test]
fn cron_usage_carries_canonical_command_and_target_alias() {
    // Standalone test on just the cron section. Catches "we forgot to
    // update the example after CLI changed flag names" — if Lane B ever
    // renames --schedule / --target / --prompt or drops the @self alias,
    // this breaks before agents ever see the stale text.
    //
    // We reach the section through any provider's default trait method
    // rather than calling `prompts::default_cron_usage` directly, because
    // the `prompts` module is `pub(crate)`. The default impl delegates to
    // the same function, so flag/alias regressions still surface here.
    let provider = gitim_agent_provider::create("mock", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "any-bot",
        model: None,
    };
    let section = provider.prompt_cron_usage(&ctx);

    assert!(section.contains("gitim cron create"), "missing canonical command");
    assert!(section.contains("--schedule"), "missing --schedule flag");
    assert!(section.contains("--target"), "missing --target flag");
    assert!(section.contains("--prompt"), "missing --prompt flag");
    assert!(section.contains("@self"), "missing @self target alias");

    // Schedule format coverage — 5-field cron + at least one alias so
    // the agent knows both forms are accepted.
    assert!(section.contains("5 字段"), "missing 5-field cron explanation");
    assert!(section.contains("@daily"), "missing @daily alias example");

    // Wake-up shape — the agent has to recognize that a [@system]
    // message with `cron(<name>):` prefix IS the trigger.
    assert!(section.contains("[@system]"), "missing system author cue");
    assert!(section.contains("cron("), "missing cron(<name>) prefix cue");

    // Discoverability commands the agent needs to know exist.
    assert!(section.contains("gitim cron list"), "missing list command");
}

#[test]
fn provider_can_override_cron_usage_to_empty() {
    let provider = CronlessProvider;
    let ctx = PromptContext {
        handler: "shellless-bot",
        model: None,
    };
    let prompt = provider.build_system_prompt(&ctx);

    // Section header is gone — proves the override took effect end-to-end
    // through build_system_prompt (not just at the trait method level).
    assert!(
        !prompt.contains("## 周期任务"),
        "cron section header should be absent when override returns empty"
    );
    assert!(
        !prompt.contains("gitim cron create"),
        "cron command example should be absent when override returns empty"
    );

    // Other sections still default — proves we didn't accidentally take
    // out more than the cron block.
    assert!(prompt.contains("你是 shellless-bot"));
    assert!(prompt.contains("## GitIM 工具"));
    assert!(prompt.contains("## 主机操作边界"));
}

#[test]
fn default_memory_uses_agents_md() {
    // Default memory text references AGENTS.md — that's the industry-wide
    // baseline most coding agents understand. Claude is the only provider
    // that swaps it to CLAUDE.md.
    let provider = gitim_agent_provider::create("mock", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };
    let memory = provider.prompt_memory(&ctx);
    assert!(memory.contains("AGENTS.md"));
    assert!(!memory.contains("CLAUDE.md"));

    let cold_start = provider.prompt_cold_start(&ctx);
    assert!(cold_start.contains("AGENTS.md"));
    assert!(!cold_start.contains("CLAUDE.md"));

    let identity = provider.prompt_identity(&ctx);
    assert!(identity.contains("AGENTS.md"));
    assert!(!identity.contains("CLAUDE.md"));
}

#[test]
fn claude_provider_uses_claude_md() {
    // Claude provider rewrites the default file name back to CLAUDE.md so the
    // agent reads/writes the conventional Claude memory file.
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };

    let memory = provider.prompt_memory(&ctx);
    assert!(memory.contains("CLAUDE.md"));
    assert!(!memory.contains("AGENTS.md"));

    let cold_start = provider.prompt_cold_start(&ctx);
    assert!(cold_start.contains("CLAUDE.md"));
    assert!(!cold_start.contains("AGENTS.md"));

    let identity = provider.prompt_identity(&ctx);
    assert!(identity.contains("CLAUDE.md"));
    assert!(!identity.contains("AGENTS.md"));

    let reset = provider.prompt_reset_protocol(&ctx);
    assert!(reset.contains("CLAUDE.md"));
    assert!(!reset.contains("AGENTS.md"));
}

#[test]
fn override_replaces_single_section() {
    let provider = TestOverrideProvider;
    let ctx = PromptContext {
        handler: "codex-bot",
        model: Some("o3"),
    };
    let prompt = provider.build_system_prompt(&ctx);

    // The override stub appears.
    assert!(prompt.contains("这是被 override 的最小记忆段"));

    // Default memory's signature phrasing is gone — proves the override took effect.
    assert!(!prompt.contains("它是你的记忆主入口"));

    // Other sections still use defaults
    assert!(prompt.contains("你是 codex-bot"));
    assert!(prompt.contains("## 对话风格"));
    assert!(prompt.contains("## GitIM 工具"));
}

#[test]
fn prompt_context_handler_is_interpolated() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "my-agent",
        model: None,
    };
    let identity = provider.prompt_identity(&ctx);
    assert!(identity.contains("你是 my-agent"));
}

#[test]
fn gitim_api_exposes_card_and_archive_commands() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };
    let api = provider.prompt_gitim_api(&ctx);

    // Card base commands
    assert!(api.contains("gitim card create"));
    assert!(api.contains("gitim card ls"));
    assert!(api.contains("gitim card read"));
    assert!(api.contains("gitim card comment"));
    assert!(api.contains("gitim card update"));

    // Card archive triplet
    assert!(api.contains("gitim card archive"));
    assert!(api.contains("gitim card unarchive"));
    assert!(api.contains("gitim card archived"));

    // Safe multi-line message input
    assert!(api.contains("gitim send <channel> --stdin"));
    assert!(api.contains("gitim dm send <handler> --stdin"));
    assert!(api.contains("gitim card comment <channel> <card_id> --stdin"));
    assert!(api.contains("heredoc + `--stdin`"));

    // CLI fallback must stay on the supported surface even when shell PATH is broken.
    assert!(api.contains(".gitim/bin/gitim"));
    assert!(api.contains("不要直接写 `.thread`"));
    assert!(api.contains("不要直接写 `.gitim/index.db`"));

    // Channel archive triplet
    assert!(api.contains("gitim archive-channel"));
    assert!(api.contains("gitim unarchive-channel"));
    assert!(api.contains("gitim archived-channels"));
}

#[test]
fn gitim_api_exposes_board_commands() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };

    let identity = provider.prompt_identity(&ctx);
    assert!(identity.contains("gitim board"));

    let api = provider.prompt_gitim_api(&ctx);
    assert!(api.contains("gitim board path"));
    assert!(api.contains("gitim board init"));
    assert!(api.contains("gitim board publish"));
    assert!(api.contains("gitim board set"));
    assert!(api.contains("gitim board section set"));
    assert!(api.contains("gitim board show <handler>"));
}

#[test]
fn gitim_api_exposes_message_body_markers() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };

    let api = provider.prompt_gitim_api(&ctx);
    assert!(api.contains("### 消息正文协议标记"));
    assert!(api.contains("<@handler>"));
    assert!(api.contains("裸写 `@handler`"));
    assert!(api.contains("<#channel>"));
    assert!(api.contains("<#channel:L000042>"));
    assert!(api.contains("<~handler>"));
    assert!(api.contains("<!https://example.com|显示文本>"));
}

#[test]
fn reset_protocol_handles_lost_gitim_output_contract() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };
    let reset = provider.prompt_reset_protocol(&ctx);

    assert!(
        reset.contains("不确定如何用 `gitim send`"),
        "reset protocol should cover a lost gitim send contract"
    );
    assert!(
        reset.contains("普通回复里写了对外消息"),
        "reset protocol should cover accidental plain assistant replies"
    );
    assert!(
        reset.contains("未调用 gitim CLI"),
        "reset protocol should require reset when the CLI contract is missing"
    );
    assert!(
        reset.contains("[[RESET]]"),
        "reset protocol should still point to the runtime reset marker"
    );
}

#[test]
fn codex_provider_uses_agents_md() {
    // Codex inherits the default — AGENTS.md is the conventional file name
    // for Codex CLI and most non-Claude coding agents.
    let provider = gitim_agent_provider::create("codex", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "codex-bot",
        model: None,
    };

    let memory = provider.prompt_memory(&ctx);
    assert!(memory.contains("AGENTS.md"));
    assert!(!memory.contains("CLAUDE.md"));

    let cold_start = provider.prompt_cold_start(&ctx);
    assert!(cold_start.contains("AGENTS.md"));
    assert!(!cold_start.contains("CLAUDE.md"));
}
