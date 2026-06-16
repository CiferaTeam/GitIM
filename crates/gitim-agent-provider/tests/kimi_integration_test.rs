#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Integration tests for `KimiProvider` over fake `kimi acp` scripts.
//!
//! These exist to pin behaviours that the inline unit tests can't reach
//! without a live ACP subprocess — specifically the plan §1333 contract
//! that a failed `session/set_model` must still surface the established
//! session id on `ExecResult.session_token`. Modelled on
//! `claude_integration_test.rs` (fixture-script + `create("kimi", …)`).
//!
//! Reference: multica/server/pkg/agent/kimi_test.go::
//! TestKimiBackendSetModelFailureFailsTask covers the same contract on
//! the Go side.

use std::collections::HashMap;
use std::process::Command;
use std::time::Duration;

use gitim_agent_provider::{create, Event, ExecOptions, ExecStatus, ProviderConfig};

/// Smoke test against the real `kimi` binary on the host machine.
///
/// This test is ignored by default because it requires a working Kimi
/// Code CLI installation and consumes a small amount of API quota. Run
/// it manually to verify that the current `kimi` provider still speaks
/// the same ACP dialect as the installed CLI:
///
/// ```bash
/// cargo test -p gitim-agent-provider --test kimi_integration_test -- --ignored
/// ```
///
/// The test skips gracefully if `kimi` is not on PATH.
#[tokio::test]
#[ignore = "requires real kimi CLI and consumes API quota"]
async fn real_kimi_hello_smoke_test() {
    let bin = "kimi";
    let version_ok = Command::new(bin)
        .arg("--version")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .is_some();
    if !version_ok {
        return;
    }

    let provider = create("kimi", ProviderConfig::default()).unwrap();
    let session = provider
        .execute(
            "Reply with exactly the word 'pong' and nothing else.",
            ExecOptions {
                timeout: Some(Duration::from_secs(60)),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let mut events = session.events;
    let mut saw_text = false;
    while let Some(event) = events.recv().await {
        if matches!(event, Event::Text { .. }) {
            saw_text = true;
        }
    }

    let result = session.result.await.expect("result channel closed early");
    assert!(
        saw_text,
        "expected at least one Text event from real kimi; got status={:?}",
        result.status
    );
    assert_eq!(
        result.status,
        ExecStatus::Completed,
        "real kimi hello should complete; error={:?}",
        result.error
    );
    assert!(
        result.session_token.is_some(),
        "real kimi should return a session_token"
    );
}

fn fixture_config(script: &str) -> ProviderConfig {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    ProviderConfig {
        executable_path: Some(format!("{manifest_dir}/tests/fixtures/{script}")),
        env: HashMap::new(),
    }
}

/// Plan §1333 contract: when `session/set_model` returns a JSON-RPC
/// error mid-handshake, the task must end in `Failed` AND carry the
/// already-established session id, so the runtime can stamp it onto
/// `AgentState.session_token` and the user's next turn (with a
/// corrected model) resumes the same conversation.
///
/// The earlier implementation used `?` short-circuit inside the
/// handshake closure, dropping the locally-bound `sid` — this test
/// regresses on that path.
#[tokio::test]
async fn set_session_model_failure_preserves_session_id() {
    let provider = create("kimi", fixture_config("mock_kimi_set_model_fails.sh")).unwrap();
    let session = provider
        .execute(
            "prompt-ignored",
            ExecOptions {
                model: Some("bogus-model".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Drain the event stream so the driver task can progress to result.
    let mut events = session.events;
    while events.recv().await.is_some() {}

    let result = session.result.await.expect("result channel closed early");
    assert_eq!(
        result.status,
        ExecStatus::Failed,
        "expected status=Failed on set_model error, got {result:?}"
    );
    assert_eq!(
        result.session_token.as_deref(),
        Some("ses_fake_kimi"),
        "session_token must survive set_model failure so the user can \
         retry with a corrected model on the same conversation; \
         got {:?}",
        result.session_token,
    );
    let err = result
        .error
        .as_deref()
        .expect("error message must be set on set_model failure");
    assert!(
        err.contains("could not switch to model \"bogus-model\""),
        "error must name the requested model, got: {err}"
    );
    assert!(
        err.contains("model not available"),
        "error must surface the upstream JSON-RPC message, got: {err}"
    );
}

#[tokio::test]
async fn executable_not_found_returns_error() {
    let config = ProviderConfig {
        executable_path: Some("/nonexistent/kimi-xyz".to_string()),
        env: HashMap::new(),
    };
    let provider = create("kimi", config).unwrap();
    let result = provider.execute("test", ExecOptions::default()).await;
    assert!(result.is_err(), "execute must error when binary is missing");
}
