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

use gitim_agent_provider::{create, ExecOptions, ExecStatus, ProviderConfig};

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
    assert!(
        result.is_err(),
        "execute must error when binary is missing"
    );
}
