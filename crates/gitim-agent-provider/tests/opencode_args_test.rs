use gitim_agent_provider::opencode::build_invocation;
use gitim_agent_provider::ExecOptions;
use std::time::Duration;

#[test]
fn no_system_prompt_no_agent_flag() {
    let opts = ExecOptions {
        system_prompt: None,
        model: None,
        resume_token: None,
        timeout: Some(Duration::from_secs(60)),
        ..Default::default()
    };
    let inv = build_invocation("hello", &opts);
    assert!(inv.args.contains(&"run".to_string()));
    assert!(inv.args.contains(&"--format".to_string()));
    assert!(inv.args.contains(&"json".to_string()));
    assert!(inv
        .args
        .contains(&"--dangerously-skip-permissions".to_string()));
    assert!(!inv.args.iter().any(|a| a == "--agent"));
    assert!(!inv.args.iter().any(|a| a == "--model"));
    assert!(!inv.env.contains_key("OPENCODE_CONFIG_CONTENT"));
    assert_eq!(
        inv.env.get("OPENCODE_PERMISSION").map(String::as_str),
        Some(r#"{"*":"allow"}"#)
    );
    // message terminator + message as last positional
    let dash_pos = inv.args.iter().position(|a| a == "--").expect("-- present");
    assert_eq!(inv.args[dash_pos + 1], "hello");
}

#[test]
fn system_prompt_injects_gitim_agent_via_env() {
    let opts = ExecOptions {
        system_prompt: Some("you are gitim".to_string()),
        model: None,
        ..Default::default()
    };
    let inv = build_invocation("hello", &opts);
    let cfg = inv
        .env
        .get("OPENCODE_CONFIG_CONTENT")
        .expect("config content set");
    let parsed: serde_json::Value = serde_json::from_str(cfg).unwrap();
    assert_eq!(parsed["agent"]["gitim"]["prompt"], "you are gitim");
    assert_eq!(parsed["agent"]["gitim"]["mode"], "primary");
    let idx = inv
        .args
        .iter()
        .position(|a| a == "--agent")
        .expect("--agent flag");
    assert_eq!(inv.args[idx + 1], "gitim");
}

#[test]
fn model_only_when_provided() {
    let with = ExecOptions {
        model: Some("anthropic/claude-sonnet-4-6".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("x", &with);
    let idx = inv
        .args
        .iter()
        .position(|a| a == "--model")
        .expect("--model flag");
    assert_eq!(inv.args[idx + 1], "anthropic/claude-sonnet-4-6");
}

#[test]
fn resume_token_uses_session_flag() {
    let opts = ExecOptions {
        resume_token: Some("ses_abc123".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("x", &opts);
    let idx = inv
        .args
        .iter()
        .position(|a| a == "--session")
        .expect("--session flag");
    assert_eq!(inv.args[idx + 1], "ses_abc123");
}

#[test]
fn prompt_flag_never_present() {
    // Regression: opencode run has no --prompt flag. Asserting absence.
    let opts = ExecOptions {
        system_prompt: Some("sys".to_string()),
        ..Default::default()
    };
    let inv = build_invocation("user msg", &opts);
    assert!(
        !inv.args.iter().any(|a| a == "--prompt"),
        "opencode run has no --prompt flag; system prompt must go through --agent + OPENCODE_CONFIG_CONTENT"
    );
}
