mod common;

use gitim_runtime::ClaudeSession;
use common::short_tempdir;

/// Real claude -p integration test. Requires `claude` CLI in PATH.
/// Run with: cargo test -p gitim-runtime --test claude -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_claude_session_roundtrip() {
    let tmp = short_tempdir();

    let mut session = ClaudeSession::new(
        "You are a test agent. Always reply in exactly one short sentence.".into(),
        "",
        tmp.path(),
    )
    .with_model("claude-sonnet-4-6");

    // First call: creates session
    let result1 = session.send("Say hello.").await.unwrap();
    eprintln!("[1st call] session_id: {}", result1.session_id);
    eprintln!("[1st call] text: {}", result1.text);
    assert!(!result1.text.is_empty(), "should get a response");
    assert!(!result1.session_id.is_empty(), "should get session_id");

    // Verify session_id is stored
    assert_eq!(
        session.session_id().unwrap(),
        result1.session_id,
        "session should store the session_id"
    );

    // Second call: resume session
    let result2 = session
        .send("What did I just ask you?")
        .await
        .unwrap();
    eprintln!("[2nd call] session_id: {}", result2.session_id);
    eprintln!("[2nd call] text: {}", result2.text);
    assert_eq!(
        result1.session_id, result2.session_id,
        "should reuse same session_id"
    );
    assert!(!result2.text.is_empty(), "should get a response");
}
