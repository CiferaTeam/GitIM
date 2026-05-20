use std::collections::HashMap;

use gitim_agent_provider::{create, ExecOptions, ExecStatus, ProviderConfig};

fn fixture_config(script: &str) -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/{script}")),
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn accumulates_usage_from_tool_use_and_final_turns() {
    let provider = create("pi", fixture_config("mock_pi_multi_turn_usage.sh")).unwrap();
    let session = provider
        .execute("prompt-ignored", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.expect("result channel closed early");
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.session_token.as_deref(), Some("pi-session-1"));

    let usage = result.usage.expect("usage should be reported");
    assert_eq!(usage.input_tokens, Some(600));
    assert_eq!(usage.output_tokens, Some(60));
    assert_eq!(usage.cache_read_tokens, Some(6000));
    assert_eq!(usage.cache_creation_tokens, Some(6));
    assert_eq!(
        usage.context_tokens,
        Some(3333),
        "billing counters should sum all turns, but context should come from the final assistant turn"
    );
}
