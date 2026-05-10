//! Integration tests for `hermes_llm::list_providers`.
//!
//! All tests use `tempfile::TempDir` for fixture isolation and run without any
//! external binary or network dependency.

use std::fs;

use gitim_runtime::hermes_llm::{list_providers, ApiProtocol, LlmProvider, ProviderKind};
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

fn make_hermes_home() -> TempDir {
    TempDir::new().expect("TempDir::new")
}

fn write_env(dir: &TempDir, content: &str) {
    fs::write(dir.path().join(".env"), content).expect("write .env");
}

fn write_config_yaml(dir: &TempDir, content: &str) {
    fs::write(dir.path().join("config.yaml"), content).expect("write config.yaml");
}

fn provider_by_id<'a>(providers: &'a [LlmProvider], id: &str) -> Option<&'a LlmProvider> {
    providers.iter().find(|p| p.id == id)
}

// ── test 1 ───────────────────────────────────────────────────────────────────

#[test]
fn empty_hermes_home_returns_empty_list() {
    // hermes_home does not exist at all
    let tmp = TempDir::new().unwrap();
    let nonexistent = tmp.path().join("does_not_exist");
    let providers = list_providers(&nonexistent);
    assert!(
        providers.is_empty(),
        "expected empty list for non-existent hermes_home, got {providers:#?}"
    );
}

// ── test 2 ───────────────────────────────────────────────────────────────────

#[test]
fn env_with_key_lists_provider() {
    // KIMI_API_KEY with non-sk-kimi prefix → should resolve to moonshot URL
    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=mk_xxxxxxxx\n");

    let providers = list_providers(tmp.path());
    let kimi = provider_by_id(&providers, "kimi-coding")
        .expect("kimi-coding should be in the list");

    assert_eq!(kimi.base_url.as_deref(), Some("https://api.moonshot.ai/v1"));
    assert_eq!(kimi.kind, ProviderKind::ApiKey);
}

// ── test 3 ───────────────────────────────────────────────────────────────────

#[test]
fn env_with_alias_lists_provider() {
    // ZAI_API_KEY is an alias for the "zai" provider
    let tmp = make_hermes_home();
    write_env(&tmp, "ZAI_API_KEY=foo\n");

    let providers = list_providers(tmp.path());
    assert!(
        provider_by_id(&providers, "zai").is_some(),
        "zai provider should appear when ZAI_API_KEY is set; got {providers:#?}"
    );
}

// ── test 4 ───────────────────────────────────────────────────────────────────

#[test]
fn empty_value_treated_as_unconfigured() {
    // KIMI_API_KEY= (empty) must NOT list kimi-coding
    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=\n");

    let providers = list_providers(tmp.path());
    assert!(
        provider_by_id(&providers, "kimi-coding").is_none(),
        "kimi-coding should NOT appear for an empty key value; got {providers:#?}"
    );
}

// ── test 5 ───────────────────────────────────────────────────────────────────

#[test]
fn config_yaml_custom_providers_listed() {
    let tmp = make_hermes_home();
    write_config_yaml(
        &tmp,
        "custom_providers:\n  - name: my-glm\n    base_url: https://x\n",
    );

    let providers = list_providers(tmp.path());
    let custom = provider_by_id(&providers, "custom:my-glm")
        .expect("custom:my-glm should appear");

    assert_eq!(custom.kind, ProviderKind::Custom);
    assert_eq!(custom.base_url.as_deref(), Some("https://x"));
    assert_eq!(custom.api_protocol, ApiProtocol::OpenAI);
}

// ── test 6 ───────────────────────────────────────────────────────────────────

#[test]
fn config_yaml_parse_error_skipped() {
    // Invalid YAML in config.yaml must not panic; builtins still come through
    // if any env keys are set.
    let tmp = make_hermes_home();
    write_env(&tmp, "DEEPSEEK_API_KEY=sk-deep\n");
    write_config_yaml(&tmp, "this: is: not: valid: yaml: [\n");

    let providers = list_providers(tmp.path());

    // Should NOT panic and should still list the builtin
    assert!(
        provider_by_id(&providers, "deepseek").is_some(),
        "deepseek builtin should still appear despite bad config.yaml; got {providers:#?}"
    );
    // Must not contain any custom providers (parsing failed)
    assert!(
        providers.iter().all(|p| p.kind != ProviderKind::Custom),
        "no custom providers expected after parse failure; got {providers:#?}"
    );
}

// ── test 7 ───────────────────────────────────────────────────────────────────

#[test]
fn builtin_and_custom_with_same_name_both_listed() {
    // .env has KIMI_API_KEY AND config.yaml has custom_providers[name=kimi-coding]
    // Both should appear because their ids differ ("kimi-coding" vs "custom:kimi-coding").
    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=mk_xxx\n");
    write_config_yaml(
        &tmp,
        "custom_providers:\n  - name: kimi-coding\n    base_url: https://custom.example.com\n",
    );

    let providers = list_providers(tmp.path());
    assert!(
        provider_by_id(&providers, "kimi-coding").is_some(),
        "builtin kimi-coding missing; got {providers:#?}"
    );
    assert!(
        provider_by_id(&providers, "custom:kimi-coding").is_some(),
        "custom:kimi-coding missing; got {providers:#?}"
    );
}

// ── test 8 ───────────────────────────────────────────────────────────────────

#[test]
fn ordering_builtin_alphabetic_then_custom() {
    let tmp = make_hermes_home();
    // Set two builtins out of alphabetical order via env to confirm sorting
    write_env(&tmp, "ZAI_API_KEY=foo\nDEEPSEEK_API_KEY=bar\n");
    write_config_yaml(
        &tmp,
        "custom_providers:\n  - name: zzz\n    base_url: https://zzz\n  - name: aaa\n    base_url: https://aaa\n",
    );

    let providers = list_providers(tmp.path());

    // Extract positions
    let pos_deepseek = providers.iter().position(|p| p.id == "deepseek").expect("deepseek");
    let pos_zai = providers.iter().position(|p| p.id == "zai").expect("zai");
    let pos_zzz = providers.iter().position(|p| p.id == "custom:zzz").expect("custom:zzz");
    let pos_aaa = providers.iter().position(|p| p.id == "custom:aaa").expect("custom:aaa");

    // deepseek < zai (alphabetic builtins)
    assert!(pos_deepseek < pos_zai, "builtins must be alphabetically ordered");
    // all builtins before any custom
    assert!(pos_deepseek < pos_zzz, "builtins must precede custom providers");
    assert!(pos_zai < pos_zzz, "builtins must precede custom providers");
    // custom providers maintain yaml order (zzz before aaa in yaml, so zzz < aaa in list)
    assert!(pos_zzz < pos_aaa, "custom providers must preserve yaml order");
}

// ── test 9 ───────────────────────────────────────────────────────────────────

#[test]
fn kimi_with_sk_kimi_prefix_resolves_coding_url() {
    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=sk-kimi-abc123\n");

    let providers = list_providers(tmp.path());
    let kimi = provider_by_id(&providers, "kimi-coding")
        .expect("kimi-coding should be in the list");

    assert_eq!(
        kimi.base_url.as_deref(),
        Some("https://api.kimi.com/coding/v1"),
        "sk-kimi-* prefix must resolve to the coding endpoint"
    );
}

// ── test 10 ──────────────────────────────────────────────────────────────────

#[test]
fn kimi_with_other_prefix_keeps_moonshot_url() {
    let tmp = make_hermes_home();
    write_env(&tmp, "KIMI_API_KEY=mk-abc123\n");

    let providers = list_providers(tmp.path());
    let kimi = provider_by_id(&providers, "kimi-coding")
        .expect("kimi-coding should be in the list");

    assert_eq!(
        kimi.base_url.as_deref(),
        Some("https://api.moonshot.ai/v1"),
        "non-sk-kimi- prefix must keep the default moonshot URL"
    );
}

// ── test 11 ──────────────────────────────────────────────────────────────────

#[test]
fn minimax_protocol_propagates_to_list_provider() {
    let tmp = make_hermes_home();
    write_env(&tmp, "MINIMAX_API_KEY=foo\n");

    let providers = list_providers(tmp.path());
    let minimax = provider_by_id(&providers, "minimax")
        .expect("minimax should appear");

    assert_eq!(
        minimax.api_protocol,
        ApiProtocol::Anthropic,
        "minimax api_protocol must be Anthropic"
    );
}
