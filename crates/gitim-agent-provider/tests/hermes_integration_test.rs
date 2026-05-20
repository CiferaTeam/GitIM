//! Integration tests for `HermesProvider` over fake `hermes acp` scripts.

use std::collections::HashMap;
use std::time::Duration;

use gitim_agent_provider::{create, ExecOptions, ExecStatus, ProviderConfig};
use tokio::time::timeout;

fn fixture_config(script: &str) -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/{script}")),
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn completed_prompt_does_not_wait_forever_for_acp_server_exit() {
    let provider = create("hermes", fixture_config("mock_hermes_acp_holds_stdout.sh")).unwrap();
    let session = provider
        .execute("prompt", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    let drain_events = tokio::spawn(async move { while events.recv().await.is_some() {} });

    let result = timeout(Duration::from_secs(5), session.result)
        .await
        .expect("provider result should not hang after prompt response")
        .expect("result channel closed early");

    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.session_token.as_deref(), Some("ses_fake_hermes"));
    assert_eq!(result.output, "fake hermes ok");
    let usage = result
        .usage
        .expect("prompt response usage must be captured");
    assert_eq!(usage.input_tokens, Some(12));
    assert_eq!(usage.output_tokens, Some(8));
    assert_eq!(usage.cache_read_tokens, Some(100));

    timeout(Duration::from_secs(1), drain_events)
        .await
        .expect("event stream should close after provider result")
        .expect("event drain task panicked");
}
