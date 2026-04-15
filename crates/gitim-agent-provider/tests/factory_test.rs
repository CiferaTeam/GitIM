use gitim_agent_provider::create;

#[test]
fn create_claude_returns_ok() {
    assert!(create("claude", Default::default()).is_ok());
}

#[test]
fn create_codex_returns_ok() {
    assert!(create("codex", Default::default()).is_ok());
}

#[test]
fn create_cursor_returns_ok() {
    assert!(create("cursor", Default::default()).is_ok());
}

#[test]
fn create_opencode_returns_ok() {
    assert!(create("opencode", Default::default()).is_ok());
}

#[test]
fn create_gemini_returns_ok() {
    assert!(create("gemini", Default::default()).is_ok());
}

#[test]
fn create_unknown_returns_error() {
    let result = create("not-a-real-provider", Default::default());
    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains("unknown provider"), "got: {msg}");
        }
        Ok(_) => panic!("expected error for unknown provider"),
    }
}
