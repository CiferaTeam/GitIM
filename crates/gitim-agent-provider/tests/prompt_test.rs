use async_trait::async_trait;
use gitim_agent_provider::{
    ExecOptions, PromptContext, Provider, ProviderConfig, ProviderError, Session,
};

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
}

#[test]
fn default_memory_references_claude_md() {
    let provider = gitim_agent_provider::create("claude", ProviderConfig::default()).unwrap();
    let ctx = PromptContext {
        handler: "bot",
        model: None,
    };
    let memory = provider.prompt_memory(&ctx);
    assert!(memory.contains("CLAUDE.md"));
}

#[test]
fn override_replaces_single_section() {
    let provider = TestOverrideProvider;
    let ctx = PromptContext {
        handler: "codex-bot",
        model: Some("o3"),
    };
    let prompt = provider.build_system_prompt(&ctx);

    // Overridden memory section references AGENTS.md
    assert!(prompt.contains("AGENTS.md"));

    // Default memory wording ("它是你的记忆文件" paired with CLAUDE.md) is gone —
    // verify by checking the exact default phrasing is absent.
    assert!(!prompt.contains("你的工作目录下有 `CLAUDE.md`，它是你的记忆文件"));

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

    // Channel archive triplet
    assert!(api.contains("gitim archive-channel"));
    assert!(api.contains("gitim unarchive-channel"));
    assert!(api.contains("gitim archived-channels"));
}

#[test]
fn codex_provider_uses_agents_md() {
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
