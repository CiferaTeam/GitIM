use std::collections::HashMap;
use std::path::PathBuf;

use gitim_agent_provider::{create, Event, ExecOptions, ExecStatus, ProviderConfig};

fn mock_config() -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/mock_codex.sh")),
        env: HashMap::new(),
    }
}

#[tokio::test]
async fn execute_and_resume_return_completed_with_thread_id() {
    let provider = create("codex", mock_config()).unwrap();
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let session1 = provider
        .execute(
            "hello",
            ExecOptions {
                cwd: Some(cwd.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut events1 = session1.events;
    let mut saw_text = false;
    while let Some(event) = events1.recv().await {
        if matches!(event, Event::Text { .. }) {
            saw_text = true;
        }
    }
    assert!(saw_text, "should have received a Text event");

    let result1 = session1.result.await.unwrap();
    assert_eq!(result1.status, ExecStatus::Completed);
    assert_eq!(result1.output, "Hello from mock codex!");
    assert_eq!(result1.session_token.as_deref(), Some("mock-codex-thread"));

    let session2 = provider
        .execute(
            "follow-up",
            ExecOptions {
                cwd: Some(cwd),
                resume_token: result1.session_token.clone(),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut events2 = session2.events;
    let mut saw_resumed_text = false;
    while let Some(event) = events2.recv().await {
        if let Event::Text { content } = event {
            saw_resumed_text = content == "Resumed mock codex thread";
        }
    }
    assert!(saw_resumed_text, "should have received resumed Text event");

    let result2 = session2.result.await.unwrap();
    assert_eq!(result2.status, ExecStatus::Completed);
    assert_eq!(result2.output, "Resumed mock codex thread");
    assert_eq!(result2.session_token.as_deref(), Some("mock-codex-thread"));
}
