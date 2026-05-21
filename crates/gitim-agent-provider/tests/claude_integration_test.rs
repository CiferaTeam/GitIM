#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::time::Duration;

use gitim_agent_provider::{create, Event, ExecOptions, ExecStatus, ProviderConfig};

fn mock_config() -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/mock_claude.sh")),
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn happy_path_returns_completed_with_output() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("happy", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    let mut saw_text = false;
    while let Some(event) = events.recv().await {
        if matches!(event, Event::Text { .. }) {
            saw_text = true;
        }
    }
    assert!(saw_text, "should have received a Text event");

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.output, "Hello from mock claude!");
    assert!(result.session_token.is_some());
    assert!(result.error.is_none());
}

#[tokio::test]
async fn error_result_returns_failed() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("error", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Failed);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn process_crash_returns_failed() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("crash", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Failed);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn timeout_returns_timeout_status() {
    let provider = create("claude", mock_config()).unwrap();
    let opts = ExecOptions {
        timeout: Some(Duration::from_millis(500)),
        ..Default::default()
    };
    let session = provider.execute("slow", opts).await.unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Timeout);
    assert!(result.error.is_some());
}

#[tokio::test]
async fn abort_stops_execution() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("slow", ExecOptions::default())
        .await
        .unwrap();

    session.abort();

    // Either we get a result or the channel errors (task killed before sending)
    let result = session.result.await;
    assert!(
        result.is_err()
            || result
                .as_ref()
                .is_ok_and(|r| r.status == ExecStatus::Aborted || r.status == ExecStatus::Failed)
    );
}

#[tokio::test]
async fn executable_not_found_returns_error() {
    let config = ProviderConfig {
        executable_path: Some("/nonexistent/claude".to_string()),
        env: HashMap::new(),
    };
    let provider = create("claude", config).unwrap();
    let result = provider.execute("test", ExecOptions::default()).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn tool_use_events_are_streamed() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("tools", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    let mut saw_tool_use = false;
    let mut saw_tool_result = false;
    while let Some(event) = events.recv().await {
        match event {
            Event::ToolUse { ref tool, .. } if tool == "Bash" => saw_tool_use = true,
            Event::ToolResult { .. } => saw_tool_result = true,
            _ => {}
        }
    }
    assert!(saw_tool_use, "should have received ToolUse event");
    assert!(saw_tool_result, "should have received ToolResult event");

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Completed);
}

#[tokio::test]
async fn usage_report_splits_result_billing_from_assistant_context() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("multi-usage", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Completed);

    let billing = result
        .usage_report
        .billing
        .as_ref()
        .expect("result usage should drive billing");
    assert_eq!(billing.input_tokens, Some(30));
    assert_eq!(billing.output_tokens, Some(80));
    assert_eq!(billing.cache_read_tokens, Some(30_000));
    assert_eq!(billing.cache_creation_tokens, Some(300));

    let context = result
        .usage_report
        .context
        .as_ref()
        .expect("assistant usage should drive context");
    assert_eq!(context.input_tokens, Some(2));
    assert_eq!(context.output_tokens, Some(6));
    assert_eq!(context.cache_read_tokens, Some(2_000));
    assert_eq!(context.cache_creation_tokens, Some(20));
}

#[tokio::test]
async fn control_request_auto_approved() {
    let provider = create("claude", mock_config()).unwrap();
    let session = provider
        .execute("control", ExecOptions::default())
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.output, "Approved and done");
}
