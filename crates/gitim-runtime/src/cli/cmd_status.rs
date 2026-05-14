//! `status` subcommand — aggregate runtime + workspace + agent counts.
//!
//! Composes three runtime endpoints into a single CLI-facing JSON document:
//! `GET /health` for `runtime_id` + `version`, `GET /workspaces` for the
//! workspace list, and `GET /workspaces/{slug}/agents` per workspace to sum
//! `agents_total`. The runtime doesn't expose either aggregate at /health
//! today, so the CLI does the fan-out.
//!
//! Output convention: success → JSON to stdout, exit 0; errors bubble up as
//! `CliError` and the caller in `bin/runtime.rs` maps to a non-zero exit code.

use crate::cli::dto::RuntimeStatus;
use crate::cli::http::{Client, CliError};

/// Build a `RuntimeStatus` snapshot and print it as JSON to stdout.
///
/// Endpoint composition:
///   1. `GET /health` → `runtime_id`, `version`
///   2. `GET /workspaces` → list of slugs (response is wrapped: `{workspaces: [...]}`)
///   3. For each slug: `GET /workspaces/{slug}/agents` → `{agents: [...]}`,
///      length contributes to `agents_total`
///
// agents_total via N+1 (one list-agents call per workspace). Acceptable for
// `status` since it's user-initiated, not a hot path. If profiling shows this
// is a problem, add `agents_total` to /health response instead.
pub async fn run(client: &Client) -> Result<i32, CliError> {
    let health = client.get("/health").await?;
    let runtime_id = health
        .get("runtime_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::Parse("/health missing runtime_id".to_string()))?
        .to_string();
    let version = health
        .get("version")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::Parse("/health missing version".to_string()))?
        .to_string();

    let ws_body = client.get("/workspaces").await?;
    let workspace_slugs = extract_workspace_slugs(&ws_body)?;

    let mut agents_total: usize = 0;
    for slug in &workspace_slugs {
        let path = format!("/workspaces/{slug}/agents");
        let agents_body = client.get(&path).await?;
        agents_total += count_agents(&agents_body)?;
    }

    let status = RuntimeStatus {
        runtime_id,
        version,
        // uptime_secs is hardcoded 0 — /health endpoint doesn't expose uptime yet.
        // Future task can add a `started_at: SystemTime` to RuntimeState and surface
        // (now - started_at).as_secs() here. Tracked as v2 enhancement.
        uptime_secs: 0,
        workspaces_count: workspace_slugs.len(),
        agents_total,
    };

    let out = serde_json::to_string_pretty(&status)
        .map_err(|e| CliError::Parse(format!("serialize RuntimeStatus: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// `/workspaces` returns `{workspaces: [{slug, ...}, ...]}` per the server
/// type `WorkspacesListResponse`. Pull just the slug list out — that's all
/// status needs to fan out to `/agents`.
fn extract_workspace_slugs(body: &serde_json::Value) -> Result<Vec<String>, CliError> {
    let arr = body
        .get("workspaces")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::Parse("/workspaces missing 'workspaces' array".to_string()))?;
    let mut slugs = Vec::with_capacity(arr.len());
    for entry in arr {
        let slug = entry
            .get("slug")
            .and_then(|v| v.as_str())
            .ok_or_else(|| CliError::Parse("workspace entry missing 'slug'".to_string()))?;
        slugs.push(slug.to_string());
    }
    Ok(slugs)
}

/// `/workspaces/{slug}/agents` returns `{ok: true, agents: [...]}`. We only
/// need the length — the inner agent shape is irrelevant for the aggregate.
fn count_agents(body: &serde_json::Value) -> Result<usize, CliError> {
    let arr = body
        .get("agents")
        .and_then(|v| v.as_array())
        .ok_or_else(|| CliError::Parse("agents response missing 'agents' array".to_string()))?;
    Ok(arr.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_workspace_slugs_happy_path() {
        let body = json!({
            "workspaces": [
                {"slug": "alpha", "workspace_name": "Alpha", "path": "/a"},
                {"slug": "beta", "workspace_name": "Beta", "path": "/b"},
            ]
        });
        let slugs = extract_workspace_slugs(&body).expect("parse");
        assert_eq!(slugs, vec!["alpha".to_string(), "beta".to_string()]);
    }

    #[test]
    fn extract_workspace_slugs_empty() {
        let body = json!({ "workspaces": [] });
        let slugs = extract_workspace_slugs(&body).expect("parse");
        assert!(slugs.is_empty());
    }

    #[test]
    fn extract_workspace_slugs_missing_top_key() {
        // A bare array would have been valid in some sketches but the runtime
        // wraps. If the wrapper goes missing, we want a clear Parse error, not
        // a silently-empty list.
        let body = json!([]);
        let err = extract_workspace_slugs(&body).expect_err("must error");
        assert!(matches!(err, CliError::Parse(_)));
    }

    #[test]
    fn extract_workspace_slugs_entry_missing_slug() {
        let body = json!({
            "workspaces": [
                {"workspace_name": "Anon", "path": "/x"},
            ]
        });
        let err = extract_workspace_slugs(&body).expect_err("must error");
        let msg = err.to_string();
        assert!(msg.contains("slug"), "got: {msg}");
    }

    #[test]
    fn count_agents_happy_path() {
        let body = json!({
            "ok": true,
            "agents": [
                {"id": "a", "handler": "a"},
                {"id": "b", "handler": "b"},
                {"id": "c", "handler": "c"},
            ]
        });
        assert_eq!(count_agents(&body).expect("count"), 3);
    }

    #[test]
    fn count_agents_empty() {
        let body = json!({ "ok": true, "agents": [] });
        assert_eq!(count_agents(&body).expect("count"), 0);
    }

    #[test]
    fn count_agents_missing_key() {
        let body = json!({ "ok": true });
        let err = count_agents(&body).expect_err("must error");
        assert!(matches!(err, CliError::Parse(_)));
    }
}
