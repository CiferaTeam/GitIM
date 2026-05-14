//! `runtime-id` subcommand — print just the device-bound runtime UUID.
//!
//! Thin wrapper over `GET /health`; we extract `runtime_id` and emit it as a
//! single-key JSON object so downstream parsers don't have to deal with two
//! different shapes (one for `status`, one for `runtime-id`). Stdout-only on
//! success; errors surface as `CliError` for the caller's exit-code mapping.

use crate::cli::http::{Client, CliError};

/// Fetch `/health`, extract `runtime_id`, print `{"runtime_id": "..."}` to
/// stdout. Returns `Ok(0)` on success.
pub async fn run(client: &Client) -> Result<i32, CliError> {
    let health = client.get("/health").await?;
    let runtime_id = health
        .get("runtime_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| CliError::Parse("/health missing runtime_id".to_string()))?
        .to_string();

    let out = serde_json::json!({ "runtime_id": runtime_id });
    let pretty = serde_json::to_string_pretty(&out)
        .map_err(|e| CliError::Parse(format!("serialize runtime_id payload: {e}")))?;
    println!("{pretty}");
    Ok(0)
}
