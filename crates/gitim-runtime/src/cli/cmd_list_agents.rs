//! `list-agents` subcommand — list a workspace's agents as JSON.
//!
//! Two output modes:
//!   * **default (redacted)**: deserialize each agent JSON through `AgentView`
//!     so the wire shape physically drops `repo_path`, `system_prompt`, `env`,
//!     `session_usage`, etc. The default is the "safe to log" projection — it
//!     can be pasted into a Slack thread or CI artifact without leaking
//!     secrets.
//!   * **`--detailed`**: round-trip through `agent_detail_from_value` which
//!     surfaces every field *and* runs `redact_env_secrets` over `env`. The
//!     redaction step is explicit at the call site so reviewers can see where
//!     untrusted env values get sanitized.
//!
//! Workspace selection delegates to `cli::workspace::resolve_workspace` —
//! single workspace auto-picks, multiple workspaces require `--workspace`,
//! empty list errors with a useful hint. Errors propagate as `CliError` and
//! the bin-level dispatcher maps them to exit codes.

use crate::cli::dto::{agent_detail_from_value, AgentView};
use crate::cli::http::{Client, CliError};
use crate::cli::workspace::resolve_workspace;

/// Fetch agents for the selected workspace and print as a JSON array.
///
/// Each agent object is filtered through one of two projections before
/// printing — see module docs for the redaction policy.
pub async fn run(
    client: &Client,
    workspace: Option<String>,
    detailed: bool,
) -> Result<i32, CliError> {
    let slug = resolve_workspace(client, workspace.as_deref()).await?;
    let body = client.get(&format!("/workspaces/{slug}/agents")).await?;
    let raw_agents = extract_agents_array(&body)?;

    // Both projections normalize to `Vec<serde_json::Value>` so we can share
    // the final serialize-and-print step. The typed DTOs (`AgentView` /
    // `AgentDetail`) still gate field shape — the to_value step is just a
    // type harmonizer, not a re-filtering pass.
    let projected = if detailed {
        project_detailed(raw_agents)?
    } else {
        project_redacted(raw_agents)?
            .into_iter()
            .map(|view| {
                serde_json::to_value(view)
                    .map_err(|e| CliError::Parse(format!("re-serialize AgentView: {e}")))
            })
            .collect::<Result<Vec<_>, _>>()?
    };

    let out = serde_json::to_string(&projected)
        .map_err(|e| CliError::Parse(format!("serialize agents array: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// `/workspaces/{slug}/agents` returns `{ok, agents: [...]}` per
/// `AgentsListResponse`. Unwrap to the inner array — both projection paths
/// iterate over it.
fn extract_agents_array(body: &serde_json::Value) -> Result<&Vec<serde_json::Value>, CliError> {
    body.get("agents")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::Parse("agents response missing 'agents' array".to_string()))
}

/// Default projection: every agent → `AgentView`. The serde shape of
/// `AgentView` drops the sensitive fields outright; no `redact_env_secrets`
/// needed because `env` isn't in the struct.
fn project_redacted(
    raw_agents: &[serde_json::Value],
) -> Result<Vec<AgentView>, CliError> {
    raw_agents
        .iter()
        .map(|v| {
            serde_json::from_value::<AgentView>(v.clone())
                .map_err(|e| CliError::Parse(format!("parse agent as AgentView: {e}")))
        })
        .collect()
}

/// `--detailed`: every agent → `AgentDetail` with env redacted via
/// `agent_detail_from_value` (the canonical constructor that bakes the
/// redaction step into the type's only blessed entry point).
fn project_detailed(
    raw_agents: &[serde_json::Value],
) -> Result<Vec<serde_json::Value>, CliError> {
    raw_agents
        .iter()
        .map(|v| {
            let detail = agent_detail_from_value(v)
                .map_err(|e| CliError::Parse(format!("parse agent as AgentDetail: {e}")))?;
            serde_json::to_value(detail)
                .map_err(|e| CliError::Parse(format!("re-serialize AgentDetail: {e}")))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Building block: the agents-wrapper body produced by
    /// `AgentsListResponse`. Tests parse against the same shape the runtime
    /// emits so projection logic is the only variable.
    fn sample_body_with_full_agent() -> serde_json::Value {
        json!({
            "ok": true,
            "agents": [
                {
                    "id": "agent-1",
                    "handler": "alice",
                    "display_name": "Alice",
                    "status": "idle",
                    "last_activity": null,
                    "messages_processed": 7,
                    "repo_path": "/abs/repo/alice",
                    "provider": "claude",
                    "model": "claude-opus-4-7",
                    "system_prompt": "You are helpful.",
                    "env": {
                        "API_KEY": "real-secret",
                        "DEBUG": "1"
                    },
                    "introduction": "Test agent",
                }
            ]
        })
    }

    #[test]
    fn extract_agents_array_happy_path() {
        let body = sample_body_with_full_agent();
        let arr = extract_agents_array(&body).expect("agents array");
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn extract_agents_array_missing_key() {
        let body = json!({ "ok": true });
        let err = extract_agents_array(&body).expect_err("must error");
        assert!(matches!(err, CliError::Parse(_)));
    }

    /// Default projection: sensitive fields physically absent from the
    /// serialized `AgentView` output, even when the upstream body included
    /// them. This is the "safe to log" guarantee.
    #[test]
    fn project_redacted_drops_sensitive_fields() {
        let body = sample_body_with_full_agent();
        let arr = extract_agents_array(&body).expect("array");
        let views = project_redacted(arr).expect("project");
        assert_eq!(views.len(), 1);
        let v = &views[0];
        assert_eq!(v.id, "agent-1");
        assert_eq!(v.handler, "alice");
        assert_eq!(v.status, "idle");
        assert_eq!(v.messages_processed, 7);

        // Round-trip to JSON and confirm forbidden keys are absent.
        let json = serde_json::to_value(v).expect("serialize");
        let obj = json.as_object().expect("object");
        for forbidden in ["repo_path", "system_prompt", "env", "introduction"] {
            assert!(
                !obj.contains_key(forbidden),
                "AgentView leaked {forbidden}: {obj:?}",
            );
        }
    }

    /// `--detailed`: secret-shaped env values get redacted; the rest passes
    /// through. Mirrors the dto-level unit test but goes through the
    /// command's projection helper so a regression in either layer is caught.
    #[test]
    fn project_detailed_redacts_env_secrets() {
        let body = sample_body_with_full_agent();
        let arr = extract_agents_array(&body).expect("array");
        let values = project_detailed(arr).expect("project");
        assert_eq!(values.len(), 1);

        let detail = &values[0];
        assert_eq!(detail["id"], "agent-1");
        // Non-secret fields pass through.
        assert_eq!(detail["repo_path"], "/abs/repo/alice");
        assert_eq!(detail["system_prompt"], "You are helpful.");
        // Secret-shaped key redacted, benign sibling preserved.
        let env = detail.get("env").and_then(|v| v.as_object()).expect("env");
        assert_eq!(env["API_KEY"], "<redacted>");
        assert_eq!(env["DEBUG"], "1");
    }

    /// Edge: empty agents list. Both projections must return an empty Vec
    /// rather than erroring, because a workspace with no agents is normal.
    #[test]
    fn project_handles_empty_array() {
        let body = json!({"ok": true, "agents": []});
        let arr = extract_agents_array(&body).expect("array");
        let redacted = project_redacted(arr).expect("redacted");
        assert!(redacted.is_empty());
        let detailed = project_detailed(arr).expect("detailed");
        assert!(detailed.is_empty());
    }
}
