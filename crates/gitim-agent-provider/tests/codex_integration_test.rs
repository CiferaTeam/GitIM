use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use gitim_agent_provider::{create, Event, ExecOptions, ExecStatus, ProviderConfig};

fn mock_config() -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/mock_codex.sh")),
        env: HashMap::new(),
    }
}

fn failing_mock_config() -> ProviderConfig {
    let mut config = mock_config();
    config
        .env
        .insert("MOCK_CODEX_FAIL_WITH_STDERR".to_string(), "1".to_string());
    config
}

fn require_max_effort_mock_config() -> ProviderConfig {
    let mut config = mock_config();
    config
        .env
        .insert("MOCK_CODEX_REQUIRE_MAX_EFFORT".to_string(), "1".to_string());
    config
}

fn slow_mock_config() -> ProviderConfig {
    let mut config = mock_config();
    config.env.insert(
        "MOCK_CODEX_WAIT_AFTER_THREAD_STARTED".to_string(),
        "1".to_string(),
    );
    config
}

fn live_usage_mock_config(codex_home: &PathBuf) -> ProviderConfig {
    let mut config = mock_config();
    config.env.insert(
        "MOCK_CODEX_WRITE_ROLLOUT_USAGE".to_string(),
        "1".to_string(),
    );
    config
        .env
        .insert("CODEX_HOME".to_string(), codex_home.display().to_string());
    config
}

fn temp_codex_home() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("gitim-codex-home-{}-{stamp}", std::process::id()))
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
    let usage1 = result1.usage.expect("turn.completed usage should parse");
    assert_eq!(usage1.input_tokens, Some(1));
    assert_eq!(usage1.cache_read_tokens, Some(0));
    assert_eq!(usage1.output_tokens, Some(1));

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

#[tokio::test]
async fn codex_streams_live_context_usage_from_rollout() {
    let codex_home = temp_codex_home();
    let provider = create("codex", live_usage_mock_config(&codex_home)).unwrap();
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut session = provider
        .execute(
            "hello",
            ExecOptions {
                cwd: Some(cwd),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut live_usage = None;
    while let Some(event) = session.events.recv().await {
        if let Event::Usage { session_id, usage } = event {
            live_usage = Some((session_id, usage));
            break;
        }
    }
    while session.events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    let _ = std::fs::remove_dir_all(&codex_home);

    assert_eq!(result.status, ExecStatus::Completed);
    let (session_id, usage) = live_usage.expect("live usage event should stream");
    assert_eq!(session_id, "mock-codex-thread");
    assert_eq!(usage.context_tokens, Some(1234));
    assert_eq!(usage.context_window_tokens, Some(258_400));

    let final_usage = result.usage.expect("final usage should still parse");
    assert_eq!(final_usage.input_tokens, Some(1));
    assert_eq!(final_usage.context_tokens, Some(1234));
    assert_eq!(final_usage.context_window_tokens, Some(258_400));
}

#[tokio::test]
async fn failed_codex_includes_stderr_tail() {
    let provider = create("codex", failing_mock_config()).unwrap();
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let session = provider
        .execute(
            "hello",
            ExecOptions {
                cwd: Some(cwd),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Failed);
    let error = result.error.expect("failure should include error text");
    assert!(error.contains("codex exited with status"));
    assert!(error.contains("codex stderr tail: mock codex stderr diagnostic"));
}

#[tokio::test]
async fn codex_provider_sets_xhigh_reasoning_effort() {
    let provider = create("codex", require_max_effort_mock_config()).unwrap();
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let session = provider
        .execute(
            "hello",
            ExecOptions {
                cwd: Some(cwd),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Completed);
    assert_eq!(result.session_token.as_deref(), Some("mock-codex-thread"));
}

#[tokio::test]
async fn cancelling_codex_returns_aborted_with_thread_id() {
    let provider = create("codex", slow_mock_config()).unwrap();
    let cwd = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut session = provider
        .execute(
            "hello",
            ExecOptions {
                cwd: Some(cwd),
                timeout: Some(Duration::from_secs(5)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut saw_running = false;
    while let Some(event) = session.events.recv().await {
        if matches!(event, Event::Status { .. }) {
            saw_running = true;
            session.cancel();
            break;
        }
    }
    assert!(
        saw_running,
        "should receive thread.started before cancellation"
    );

    while session.events.recv().await.is_some() {}

    let result = session.result.await.unwrap();
    assert_eq!(result.status, ExecStatus::Aborted);
    assert_eq!(result.error.as_deref(), Some("cancelled by steering"));
    assert_eq!(result.session_token.as_deref(), Some("mock-codex-thread"));
}
