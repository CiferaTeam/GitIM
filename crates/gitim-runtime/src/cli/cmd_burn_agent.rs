//! `burn-agent` subcommand — depart an agent from the workspace.
//!
//! Two flavors share this entry point:
//!
//! - Default (`hard == false`): POST `/workspaces/{slug}/agents/burn` — the
//!   archive-protocol "ritual burn" that broadcasts a workspace-wide
//!   departure event, writes audit commits via the daemon's `depart_user`
//!   RPC, then drops the agent's clone. Use this for normal goodbyes.
//! - `--hard`: POST `/workspaces/{slug}/agents/remove` with
//!   `hard_delete: true` — the legacy quiet path that only touches local
//!   state (clone + hermes profile + in-memory `ctx.agents`). No SSE
//!   broadcast, no audit commits. Reserved for cases where ritual burn
//!   can't run (broken daemon, missing remote, dev-mode resets) and the
//!   operator explicitly wants to skip the protocol.
//!
//! The runtime owns the two endpoints; the CLI just picks which one to call
//! based on the `--hard` flag and forwards the response verbatim. The flag
//! never gets translated into a body field — `/burn` doesn't accept
//! `hard_delete`, and `/remove`'s `hard_delete` is always `true` when we
//! route through this path. (A `--hard=false` against `/remove` would only
//! drop the in-memory state and leave the clone on disk, which is a footgun
//! we don't need to expose.)
//!
//! The handler accepts the agent **id** rather than handler. The two are
//! identical in practice today, but routing on id matches the wire shape of
//! both endpoints (`AgentIdRequest::id` / `AgentRemoveRequest::id`) and
//! avoids implying we'd ever do a server-side handler-to-id lookup.

use serde_json::json;

use crate::cli::http::{Client, CliError};
use crate::cli::workspace::resolve_workspace;

/// Entry point. Sequence:
///   1. Resolve workspace slug (auto-pick if exactly one, else require flag)
///   2. Build the request body via `build_burn_request` — keeps the
///      flag-to-endpoint demux pure-function testable
///   3. POST to the chosen endpoint
///   4. Print the runtime's raw response to stdout
///
/// Both endpoints respond with `{ok: true}` on success. Structured failures
/// (e.g. `not_an_agent`) come back through `CliError::ResponseErrorCode` from
/// the shared `process_response` and bubble up; we don't try to interpret
/// them here.
pub async fn run(
    client: &Client,
    workspace: Option<String>,
    id: String,
    hard: bool,
) -> Result<i32, CliError> {
    let slug = resolve_workspace(client, workspace.as_deref()).await?;
    let (path, body) = build_burn_request(&slug, &id, hard);
    let response = client.post(&path, &body).await?;
    let out = serde_json::to_string(&response)
        .map_err(|e| CliError::Parse(format!("serialize burn response: {e}")))?;
    println!("{out}");
    Ok(0)
}

/// Pure helper: pick `(path, body)` based on `--hard`.
///
/// Split out so the dispatch shape is unit-testable without spinning a
/// router. The mapping is the entire CLI-side contract of this subcommand
/// — keeping it in one inspectable function makes drift between the two
/// endpoints visible at review time.
pub(crate) fn build_burn_request(
    slug: &str,
    id: &str,
    hard: bool,
) -> (String, serde_json::Value) {
    if hard {
        (
            format!("/workspaces/{slug}/agents/remove"),
            json!({ "id": id, "hard_delete": true }),
        )
    } else {
        (
            format!("/workspaces/{slug}/agents/burn"),
            json!({ "id": id }),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Default (no `--hard`) routes to the burn endpoint with just the id —
    /// no `hard_delete` field, no extras. The runtime's `AgentIdRequest`
    /// only carries `id`, so any extra field would either be silently
    /// dropped or trigger a serde rejection depending on the version. Keep
    /// the body minimal.
    #[test]
    fn build_request_default_targets_burn() {
        let (path, body) = build_burn_request("ws", "alice", false);
        assert_eq!(path, "/workspaces/ws/agents/burn");
        let obj = body.as_object().expect("body is object");
        assert_eq!(obj["id"], "alice");
        assert!(
            !obj.contains_key("hard_delete"),
            "burn body must omit hard_delete; got: {obj:?}"
        );
    }

    /// `--hard` flips both the path AND the body shape. The remove endpoint
    /// takes `{id, hard_delete}`, and we always pass `true` because exposing
    /// `--hard=false → /remove` would just delete in-memory state and leave
    /// the clone behind — a footgun we don't need.
    #[test]
    fn build_request_hard_targets_remove() {
        let (path, body) = build_burn_request("ws", "alice", true);
        assert_eq!(path, "/workspaces/ws/agents/remove");
        assert_eq!(body["id"], "alice");
        assert_eq!(
            body["hard_delete"], true,
            "hard path always sends hard_delete: true"
        );
    }

    /// Slug interpolation isn't escaped — that's deliberate, the runtime's
    /// `WorkspaceSlug` extractor enforces the slug format and the resolver
    /// has already validated against the live workspace list before we
    /// build the path. This test just locks the URL shape so a future
    /// refactor doesn't drop the slug.
    #[test]
    fn build_request_path_carries_slug() {
        let (path, _) = build_burn_request("my-workspace", "bot", false);
        assert!(
            path.contains("/workspaces/my-workspace/"),
            "slug must appear in path: {path}"
        );
    }

    /// Agent id round-trips into the body verbatim. We don't lowercase,
    /// trim, or otherwise massage — the runtime owns id validation, and
    /// silent client-side rewrites would make debugging a 404 painful.
    #[test]
    fn build_request_id_passthrough() {
        for id in ["alice", "Bot_42", "x-y-z"] {
            let (_, body) = build_burn_request("ws", id, false);
            assert_eq!(body["id"], id, "id must round-trip verbatim");
        }
    }
}
