//! Workspace selection for CLI subcommands.
//!
//! Most workspace-scoped subcommands accept an optional `--workspace <slug>`.
//! When the user omits it, the CLI must auto-pick if exactly one workspace
//! exists, and refuse with a useful error otherwise. The logic is factored
//! into a pure function (`select_workspace`) so it's directly unit-testable
//! without spinning a mock HTTP server; `resolve_workspace` then layers the
//! HTTP fetch on top.

use serde::Deserialize;

use super::http::{Client, CliError};

/// Minimal projection of a workspace entry from `GET /workspaces`. We only
/// need `slug` for selection logic; full DTOs live in task 5's output module.
/// `#[serde(default)]` on the vec means a `null` JSON body parses as empty,
/// matching real runtime behavior on first boot.
#[derive(Debug, Deserialize)]
struct WorkspaceLite {
    slug: String,
}

/// Pure selection step — no I/O, takes the candidate list and the user's
/// optional request, returns either the chosen slug or a descriptive error.
///
/// Errors all map to `InvalidConfig` because the failure is a CLI usage
/// problem (no workspace, ambiguous selection, unknown slug), not a runtime
/// failure. That keeps exit codes consistent: every variant here exits 1.
pub fn select_workspace(
    requested: Option<&str>,
    candidates: &[String],
) -> Result<String, CliError> {
    match requested {
        Some(slug) => {
            if candidates.iter().any(|s| s == slug) {
                Ok(slug.to_string())
            } else {
                Err(CliError::InvalidConfig(format!(
                    "workspace '{slug}' not found; available: [{}]",
                    candidates.join(", ")
                )))
            }
        }
        None => match candidates.len() {
            0 => Err(CliError::InvalidConfig(
                "no workspace configured; run /git/init first or use WebUI".to_string(),
            )),
            1 => Ok(candidates[0].clone()),
            _ => Err(CliError::InvalidConfig(format!(
                "multiple workspaces, specify --workspace: [{}]",
                candidates.join(", ")
            ))),
        },
    }
}

/// Async wrapper: fetch the workspace list from the runtime and delegate to
/// `select_workspace`. Subcommand handlers call this once at the start of
/// their flow before issuing the workspace-scoped request.
pub async fn resolve_workspace(
    client: &Client,
    requested: Option<&str>,
) -> Result<String, CliError> {
    let value = client.get("/workspaces").await?;
    let entries: Vec<WorkspaceLite> = serde_json::from_value(value)
        .map_err(|e| CliError::Parse(format!("/workspaces body: {e}")))?;
    let slugs: Vec<String> = entries.into_iter().map(|w| w.slug).collect();
    select_workspace(requested, &slugs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matches_invalid_config(err: &CliError, needle: &str) -> bool {
        matches!(err, CliError::InvalidConfig(msg) if msg.contains(needle))
    }

    #[test]
    fn auto_pick_single() {
        let candidates = vec!["frontend".to_string()];
        let chosen = select_workspace(None, &candidates).expect("single workspace auto-picks");
        assert_eq!(chosen, "frontend");
    }

    #[test]
    fn auto_error_empty() {
        let candidates: Vec<String> = vec![];
        let err = select_workspace(None, &candidates).expect_err("empty list must error");
        assert!(
            matches_invalid_config(&err, "no workspace configured"),
            "got: {err}"
        );
    }

    #[test]
    fn auto_error_multiple_lists_slugs() {
        let candidates = vec!["frontend".to_string(), "backend".to_string()];
        let err =
            select_workspace(None, &candidates).expect_err("multiple workspaces must error");
        // Both slugs must appear in the message so the user can copy one
        // into --workspace without re-running anything to list them.
        let msg = err.to_string();
        assert!(msg.contains("frontend"), "got: {msg}");
        assert!(msg.contains("backend"), "got: {msg}");
        assert!(msg.contains("--workspace"), "got: {msg}");
    }

    #[test]
    fn explicit_match() {
        let candidates = vec!["frontend".to_string(), "backend".to_string()];
        let chosen = select_workspace(Some("backend"), &candidates).expect("explicit match works");
        assert_eq!(chosen, "backend");
    }

    #[test]
    fn explicit_not_found_lists_candidates() {
        let candidates = vec!["frontend".to_string(), "backend".to_string()];
        let err = select_workspace(Some("nonexistent"), &candidates)
            .expect_err("unknown slug must error");
        let msg = err.to_string();
        assert!(msg.contains("nonexistent"), "got: {msg}");
        assert!(msg.contains("frontend"), "got: {msg}");
        assert!(msg.contains("backend"), "got: {msg}");
    }

    #[test]
    fn explicit_empty_candidates() {
        // Edge case: user passed --workspace foo but no workspaces exist.
        // We report "not found" with empty list, which is more accurate than
        // "no workspace configured" — the user clearly intended one.
        let candidates: Vec<String> = vec![];
        let err = select_workspace(Some("foo"), &candidates).expect_err("must error");
        assert!(matches_invalid_config(&err, "foo"), "got: {err}");
    }
}
