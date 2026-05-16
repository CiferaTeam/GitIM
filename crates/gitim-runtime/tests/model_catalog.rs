use std::os::unix::fs::PermissionsExt;

use gitim_runtime::model_catalog::{
    list_provider_models_with_overrides, parse_codex_debug_models, parse_opencode_models,
    ModelCatalogOverrides,
};

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

#[tokio::test]
async fn codex_catalog_invokes_debug_models_and_exposes_fallbacks() {
    let tmp = tempfile::tempdir().unwrap();
    let capture = tmp.path().join("argv.txt");
    let script = make_script(
        tmp.path(),
        &format!(
            r#"#!/bin/sh
printf '%s\n' "$*" > "{}"
cat <<'JSON'
{{"models":[{{"slug":"gpt-5.5","display_name":"GPT-5.5","visibility":"list"}}]}}
JSON
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
