// Integration test — eprintln! used for test diagnostics.
#![allow(clippy::print_stderr)]

mod common;

use common::short_tempdir;
use gitim_agent_provider::{create, ExecOptions, ExecStatus, ProviderConfig};

/// Real provider integration test. Requires `claude` CLI in PATH.
/// Run with: cargo test -p gitim-runtime --test claude -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn test_provider_session_roundtrip() {
    let tmp = short_tempdir();

    let provider = create("claude", ProviderConfig::default()).unwrap();

    // First call: creates session
    let opts1 = ExecOptions {
        cwd: Some(tmp.path().to_path_buf()),
        model: Some("claude-sonnet-4-6".to_string()),
        system_prompt: Some("Always reply in exactly one short sentence.".to_string()),
        max_turns: Some(1),
        ..Default::default()
    };

    let session1 = provider.execute("Say hello.", opts1).await.unwrap();

    // Drain events
    let mut events = session1.events;
    while (events.recv().await).is_some() {}

    let result1 = session1.result.await.unwrap();
    eprintln!(
        "[1st] status={:?} output={}",
        result1.status, result1.output
    );
    eprintln!("[1st] session_token={:?}", result1.session_token);
    assert_eq!(result1.status, ExecStatus::Completed);
    assert!(!result1.output.is_empty());
    assert!(result1.session_token.is_some());

    let token = result1.session_token.unwrap();

    // Second call: resume
    let opts2 = ExecOptions {
        cwd: Some(tmp.path().to_path_buf()),
        model: Some("claude-sonnet-4-6".to_string()),
        max_turns: Some(1),
        resume_token: Some(token.clone()),
        ..Default::default()
    };

    let session2 = provider
        .execute("What did I just ask you?", opts2)
        .await
        .unwrap();

    let mut events2 = session2.events;
    while (events2.recv().await).is_some() {}

    let result2 = session2.result.await.unwrap();
    eprintln!(
        "[2nd] status={:?} output={}",
        result2.status, result2.output
    );
    eprintln!("[2nd] session_token={:?}", result2.session_token);
    assert_eq!(result2.status, ExecStatus::Completed);
    assert!(!result2.output.is_empty());

    // Session token should be the same (same session)
    assert_eq!(result2.session_token.as_deref(), Some(token.as_str()));
}
