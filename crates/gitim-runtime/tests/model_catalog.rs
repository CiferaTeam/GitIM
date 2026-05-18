use std::os::unix::fs::PermissionsExt;

use gitim_runtime::model_catalog::{
    list_provider_models_with_overrides, parse_codex_debug_models, parse_cursor_models,
    parse_kimi_session_models, parse_opencode_models, parse_pi_models, ModelCatalogOverrides,
};
use serde_json::json;

fn make_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("fake-cli.sh");
    std::fs::write(&path, body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn codex_parser_keeps_list_visible_models() {
    let input = r#"{
      "models": [
        { "slug": "gpt-5.5", "display_name": "GPT-5.5", "visibility": "list" },
        { "slug": "codex-auto-review", "display_name": "Codex Auto Review", "visibility": "hide" },
        { "slug": "gpt-next", "visibility": "list" }
      ]
    }"#;

    let models = parse_codex_debug_models(input).unwrap();

    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["gpt-5.5", "gpt-next"]);
    assert_eq!(models[0].label, "GPT-5.5");
    assert_eq!(models[1].label, "gpt-next");
}

#[test]
fn opencode_parser_reads_provider_model_lines() {
    let input =
        "\n  kimi-for-coding/k2p6\nmoonshotai/kimi-k2-thinking\ninvalid\nkimi-for-coding/k2p6\n";

    let models = parse_opencode_models(input);

    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["kimi-for-coding/k2p6", "moonshotai/kimi-k2-thinking"]
    );
}

#[test]
fn pi_parser_reads_table_rows_as_provider_model_ids() {
    let input = r#"
provider     model               context  max-out  thinking  images
deepseek     deepseek-chat       64K      8.2K     no        no
kimi-coding  kimi-for-coding     262.1K   32.8K    yes       yes
mass         astron-code-latest  200K     32K      yes       yes
kimi-coding  kimi-for-coding     262.1K   32.8K    yes       yes
"#;

    let models = parse_pi_models(input);

    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "deepseek/deepseek-chat",
            "kimi-coding/kimi-for-coding",
            "mass/astron-code-latest"
        ]
    );
    assert_eq!(models[0].label, "deepseek/deepseek-chat");
}

#[test]
fn pi_parser_returns_empty_when_no_table_rows_exist() {
    let models = parse_pi_models("No models available. Configure an API key.\n");

    assert!(models.is_empty());
}

#[test]
fn cursor_parser_reads_model_rows() {
    let input = r#"
Available models

auto - Auto
composer-2-fast - Composer 2 Fast (default)
gpt-5.3-codex - Codex 5.3
composer-2-fast - Composer 2 Fast (default)

Tip: use --model <id> to switch.
"#;

    let models = parse_cursor_models(input);

    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["auto", "composer-2-fast", "gpt-5.3-codex"]);
    assert_eq!(models[1].label, "Composer 2 Fast (default)");
}

#[test]
fn kimi_parser_reads_acp_session_models_and_marks_current_default() {
    let result = json!({
        "models": {
            "availableModels": [
                {"modelId": "kimi-code/kimi-for-coding", "name": "kimi-for-coding"},
                {"modelId": "kimi-code/kimi-for-coding,thinking", "name": "kimi-for-coding (thinking)"},
                {"modelId": "kimi-code/kimi-for-coding", "name": "duplicate"}
            ],
            "currentModelId": "kimi-code/kimi-for-coding,thinking"
        }
    });

    let models = parse_kimi_session_models(&result);

    let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![
            "kimi-code/kimi-for-coding",
            "kimi-code/kimi-for-coding,thinking"
        ]
    );
    assert_eq!(models[0].label, "kimi-for-coding");
    assert_eq!(models[1].label, "kimi-for-coding (thinking) (default)");
}

#[tokio::test]
async fn codex_catalog_invokes_debug_models_and_exposes_fallbacks() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
printf '%s\n' '{{"models":[{{"slug":"gpt-5.5","display_name":"GPT-5.5","visibility":"list"}}]}}'
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "codex",
        ModelCatalogOverrides {
            codex_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(
        std::fs::read_to_string(capture).unwrap().trim(),
        "debug models"
    );
    assert_eq!(result.provider, "codex");
    assert_eq!(result.source, "codex_debug_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models.len(), 1);
    assert_eq!(result.models[0].id, "gpt-5.5");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn pi_catalog_invokes_list_models_and_exposes_fallbacks() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
printf '%s\n' 'provider     model               context  max-out  thinking  images'
printf '%s\n' 'kimi-coding  kimi-for-coding     262.1K   32.8K    yes       yes'
printf '%s\n' 'mass         astron-code-latest  200K     32K      yes       yes'
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "pi",
        ModelCatalogOverrides {
            pi_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(
        std::fs::read_to_string(capture).unwrap().trim(),
        "--list-models"
    );
    assert_eq!(result.provider, "pi");
    assert_eq!(result.source, "pi_list_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models[0].id, "kimi-coding/kimi-for-coding");
    assert_eq!(result.models[1].id, "mass/astron-code-latest");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn pi_catalog_reads_models_from_stderr() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
cat >&2 <<'TABLE'
provider     model                   context  max-out  thinking  images
minimax-cn   MiniMax-M2.7-highspeed  204.8K   131.1K   yes       no
TABLE
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "pi",
        ModelCatalogOverrides {
            pi_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(
        std::fs::read_to_string(capture).unwrap().trim(),
        "--list-models"
    );
    assert_eq!(result.provider, "pi");
    assert_eq!(result.source, "pi_list_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models[0].id, "minimax-cn/MiniMax-M2.7-highspeed");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn opencode_catalog_invokes_models_without_refresh() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
printf '%s\n' 'kimi-for-coding/k2p6' 'moonshotai/kimi-k2-thinking'
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "opencode",
        ModelCatalogOverrides {
            opencode_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(std::fs::read_to_string(capture).unwrap().trim(), "models");
    assert_eq!(result.provider, "opencode");
    assert_eq!(result.source, "opencode_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models[0].id, "kimi-for-coding/k2p6");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn kimi_catalog_invokes_acp_session_new_and_exposes_models() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
IFS= read -r _init
printf '%s\n' '{{"jsonrpc":"2.0","id":0,"result":{{"protocolVersion":1}}}}'
IFS= read -r _new
printf '%s\n' '{{"jsonrpc":"2.0","id":1,"result":{{"models":{{"availableModels":[{{"modelId":"kimi-code/kimi-for-coding","name":"kimi-for-coding"}},{{"modelId":"kimi-code/kimi-for-coding,thinking","name":"kimi-for-coding (thinking)"}}],"currentModelId":"kimi-code/kimi-for-coding,thinking"}},"sessionId":"sid"}}}}'
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "kimi",
        ModelCatalogOverrides {
            kimi_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(
        std::fs::read_to_string(capture).unwrap().trim(),
        "--afk acp"
    );
    assert_eq!(result.provider, "kimi");
    assert_eq!(result.source, "kimi_acp_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models[0].id, "kimi-code/kimi-for-coding");
    assert_eq!(result.models[1].id, "kimi-code/kimi-for-coding,thinking");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}

#[tokio::test]
async fn cursor_catalog_invokes_models_and_exposes_fallbacks() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
printf '%s\n' 'Available models'
printf '%s\n' ''
printf '%s\n' 'auto - Auto'
printf '%s\n' 'composer-2-fast - Composer 2 Fast (default)'
printf '%s\n' 'gpt-5.3-codex - Codex 5.3'
printf '%s\n' ''
printf '%s\n' 'Tip: use --model <id> to switch.'
"#,
            capture.display()
        ),
    );

    let result = list_provider_models_with_overrides(
        "cursor",
        ModelCatalogOverrides {
            cursor_bin: Some(script.to_string_lossy().into_owned()),
            ..Default::default()
        },
    )
    .await;

    assert_eq!(std::fs::read_to_string(capture).unwrap().trim(), "models");
    assert_eq!(result.provider, "cursor");
    assert_eq!(result.source, "cursor_models");
    assert!(result.supports_default);
    assert!(result.supports_custom);
    assert_eq!(result.models[0].id, "auto");
    assert_eq!(result.models[1].id, "composer-2-fast");
    assert!(
        result.error.is_none(),
        "unexpected error: {:?}",
        result.error
    );
}
