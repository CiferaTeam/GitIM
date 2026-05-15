//! `workspaces` subcommand — list workspaces known to the runtime.
//!
//! Thin wrapper over `GET /workspaces`. Where `status` only needs the slug
//! list (for fan-out to `/agents`) and `workspace::resolve_workspace` only
//! needs slugs (for selection), this command surfaces every field the runtime
//! returns per entry (slug, workspace_name, path, provider, initialized,
//! plus any future additions). To stay forward-compatible we pass the raw
//! `serde_json::Value` array straight through to stdout instead of round-
//! tripping through a typed projection.
//!
//! Output convention: success → compact JSON array to stdout, exit 0; errors
//! bubble up as `CliError` for the caller's exit-code mapping.

use crate::cli::http::{CliError, Client};

/// Fetch `/workspaces`, unwrap the `{workspaces: [...]}` envelope, print the
/// inner array as compact JSON to stdout. Returns `Ok(0)` on success.
///
/// Compact output (`to_string`, not `to_string_pretty`) is the default so
/// scripts piping into `jq` get a single-line array. A `--pretty` flag can
/// land later if humans complain — not needed for T7.
pub async fn run(client: &Client) -> Result<i32, CliError> {
    let body = client.get("/workspaces").await?;
    let array = extract_workspaces_array(&body)?;

    let out = serde_json::to_string(&array)
        .map_err(|e| CliError::Parse(format!("serialize workspaces array: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// `/workspaces` returns `{workspaces: [...]}` per the server type
/// `WorkspacesListResponse`. Unwrap to the inner array, preserving every
/// field the server emitted so the CLI surface tracks the wire format
/// automatically.
fn extract_workspaces_array(body: &serde_json::Value) -> Result<serde_json::Value, CliError> {
    let arr = body
        .get("workspaces")
        .ok_or_else(|| CliError::Parse("/workspaces missing 'workspaces' key".to_string()))?;
    if !arr.is_array() {
        return Err(CliError::Parse(
            "/workspaces 'workspaces' field is not an array".to_string(),
        ));
    }
    Ok(arr.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extract_workspaces_array_happy_path() {
        let body = json!({
            "workspaces": [
                {"slug": "alpha", "workspace_name": "Alpha", "path": "/a", "provider": "local"},
                {"slug": "beta", "workspace_name": "Beta", "path": "/b", "provider": "github"},
            ]
        });
        let arr = extract_workspaces_array(&body).expect("parse");
        let entries = arr.as_array().expect("array");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["slug"], "alpha");
        assert_eq!(entries[1]["slug"], "beta");
        // All fields pass through — we don't filter or rename.
        assert_eq!(entries[0]["provider"], "local");
        assert_eq!(entries[1]["provider"], "github");
    }

    #[test]
    fn extract_workspaces_array_empty() {
        let body = json!({ "workspaces": [] });
        let arr = extract_workspaces_array(&body).expect("parse");
        assert!(arr.as_array().expect("array").is_empty());
    }

    #[test]
    fn extract_workspaces_array_missing_key() {
        // A bare array would be valid in some sketches but the runtime wraps.
        // Missing wrapper → clear Parse error, not silent empty list.
        let body = json!([]);
        let err = extract_workspaces_array(&body).expect_err("must error");
        assert!(matches!(err, CliError::Parse(_)));
    }

    #[test]
    fn extract_workspaces_array_wrong_type() {
        // Guard against the runtime accidentally sending `{workspaces: null}`
        // or `{workspaces: "oops"}` — surface as Parse, not silent success.
        let body = json!({ "workspaces": null });
        let err = extract_workspaces_array(&body).expect_err("must error");
        assert!(matches!(err, CliError::Parse(_)));
    }

    /// Forward-compat: any new field the runtime adds to a workspace entry
    /// should land in the output as-is. We don't model entries through a
    /// typed projection precisely so this works without code changes.
    #[test]
    fn extract_workspaces_array_preserves_unknown_fields() {
        let body = json!({
            "workspaces": [
                {
                    "slug": "alpha",
                    "workspace_name": "Alpha",
                    "path": "/a",
                    "provider": "local",
                    "initialized": true,
                    "future_field": {"nested": 42},
                }
            ]
        });
        let arr = extract_workspaces_array(&body).expect("parse");
        let entry = &arr.as_array().expect("array")[0];
        assert_eq!(entry["initialized"], true);
        assert_eq!(entry["future_field"]["nested"], 42);
    }
}
