//! Unified labels space — IPC handlers for `users/<h>.meta.yaml.labels`.
//!
//! - `handle_labels_add` / `handle_labels_remove`: self-claim only,
//!   read-modify-write under `commit_lock`, yaml rollback on commit failure
//! - `handle_labels_list`: read any active user (404 for departed)
//! - `handle_agents_with_labels`: all-of subset match across active users
//! - `compute_suggested_assignees`: best-effort helper used by
//!   `handle_create_card` to populate `CreateCardResponse.suggested_assignees`
//!
//! Spec: docs/plans/unified-labels/00-requirements.md (P4, P5, P5b)
//! Plan: docs/plans/unified-labels/01-plan.md (Phase C)

use std::collections::BTreeSet;

use gitim_core::responses::{
    AgentsWithLabelsResponse, LabelsAddResponse, LabelsListResponse, LabelsRemoveResponse,
};
use gitim_core::types::{validate_labels, UserMeta, USER_MAX_LABELS};
use tracing::warn;

use crate::api::Response;
use crate::state::SharedState;

/// `LabelsAdd` IPC handler. Self-claim only.
///
/// Flow:
/// 1. Verify caller (daemon's bound handler in `state.current_user`) == target
/// 2. Validate proposed labels (char set + per-label length)
/// 3. Acquire `commit_lock` (held across read-modify-write to avoid TOCTOU
///    between concurrent adds — eng-review Issue #3)
/// 4. Read existing `users/<target>.meta.yaml` from disk, parse, union labels
/// 5. Validate post-union cap (≤ USER_MAX_LABELS)
/// 6. Write yaml back, commit. On commit failure restore old bytes.
/// 7. Drop lock, push best-effort (sync_loop will retry).
pub async fn handle_labels_add(
    state: SharedState,
    target: String,
    labels: Vec<String>,
) -> Response {
    if let Err(resp) = ensure_self(&state, &target).await {
        return resp;
    }

    if let Err(e) = validate_labels(&labels, USER_MAX_LABELS) {
        return Response::error_with_code(format!("invalid labels: {e}"), "invalid_label");
    }

    // Acquire commit_lock for the read-modify-write window.
    let guard = state
        .commit_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let existing = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let old_bytes = existing.clone();

    let mut meta: UserMeta = match serde_yaml::from_str(&existing) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };

    // Union with existing (BTreeSet handles dedup + sort).
    let mut union: BTreeSet<String> = meta.labels.into_iter().collect();
    for l in &labels {
        union.insert(l.clone());
    }
    if union.len() > USER_MAX_LABELS {
        return Response::error_with_code(
            format!(
                "would exceed user cap {} (resulting count {})",
                USER_MAX_LABELS,
                union.len()
            ),
            "labels_full",
        );
    }
    meta.labels = union.into_iter().collect();

    let new_yaml = match Response::yaml_string(&meta, "user meta") {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(e) = std::fs::write(&meta_path, &new_yaml) {
        return Response::error(format!("write user meta failed: {e}"));
    }

    let rel_path = format!("users/{}.meta.yaml", target);
    let commit_msg = format!("user: labels add @{}", target);
    let (author_name, author_email) = state.author_for(&target);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&rel_path],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Rollback yaml to keep working tree consistent with HEAD.
        if let Err(restore_err) = std::fs::write(&meta_path, &old_bytes) {
            warn!(
                "labels_add: commit failed AND yaml rollback failed: \
                 commit_err={e}, restore_err={restore_err}"
            );
        }
        return Response::error(format!("labels_add commit failed: {e}"));
    }

    let current_labels = meta.labels;
    drop(guard);

    // Push best-effort outside the lock; sync_loop retries on failure.
    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("labels_add: push failed (commit durable, sync_loop will retry): {e}");
        }
    }

    Response::json(LabelsAddResponse { current_labels })
}

/// `LabelsRemove` IPC handler. Self-claim only. Same shape as `labels_add`
/// but set-subtraction instead of set-union.
pub async fn handle_labels_remove(
    state: SharedState,
    target: String,
    labels: Vec<String>,
) -> Response {
    if let Err(resp) = ensure_self(&state, &target).await {
        return resp;
    }

    let guard = state
        .commit_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let existing = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let old_bytes = existing.clone();

    let mut meta: UserMeta = match serde_yaml::from_str(&existing) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };

    let to_remove: BTreeSet<String> = labels.into_iter().collect();
    let mut remaining: BTreeSet<String> = meta.labels.into_iter().collect();
    for l in &to_remove {
        remaining.remove(l);
    }
    meta.labels = remaining.into_iter().collect();

    let new_yaml = match Response::yaml_string(&meta, "user meta") {
        Ok(s) => s,
        Err(resp) => return resp,
    };
    if let Err(e) = std::fs::write(&meta_path, &new_yaml) {
        return Response::error(format!("write user meta failed: {e}"));
    }

    let rel_path = format!("users/{}.meta.yaml", target);
    let commit_msg = format!("user: labels remove @{}", target);
    let (author_name, author_email) = state.author_for(&target);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&rel_path],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        if let Err(restore_err) = std::fs::write(&meta_path, &old_bytes) {
            warn!(
                "labels_remove: commit failed AND yaml rollback failed: \
                 commit_err={e}, restore_err={restore_err}"
            );
        }
        return Response::error(format!("labels_remove commit failed: {e}"));
    }

    let current_labels = meta.labels;
    drop(guard);

    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("labels_remove: push failed (commit durable, sync_loop will retry): {e}");
        }
    }

    Response::json(LabelsRemoveResponse { current_labels })
}

/// `LabelsList` IPC handler. Read any active user.
///
/// 404 with `error_code: "unknown_user"` if target is not in
/// `state.users` (covers both "never registered" and "in `archive/users/`").
pub async fn handle_labels_list(state: SharedState, target: String) -> Response {
    {
        let users = state.users.read().await;
        if !users.contains(&target) {
            return Response::error_with_code(format!("unknown user: {target}"), "unknown_user");
        }
    }

    let meta_path = state
        .repo_root
        .join("users")
        .join(format!("{}.meta.yaml", target));
    let yaml = match std::fs::read_to_string(&meta_path) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("read user meta failed: {e}")),
    };
    let meta: UserMeta = match serde_yaml::from_str(&yaml) {
        Ok(m) => m,
        Err(e) => return Response::error(format!("parse user meta failed: {e}")),
    };

    Response::json(LabelsListResponse {
        handler: target,
        labels: meta.labels,
    })
}

/// `AgentsWithLabels` IPC handler. All-of subset match.
///
/// Empty query → empty result (deliberately not "all agents").
/// Scans `users/*.meta.yaml` for each active handler from `state.users`
/// (which excludes departed handlers in `archive/users/`).
/// fs I/O is wrapped in `spawn_blocking` to avoid stalling the tokio reactor.
pub async fn handle_agents_with_labels(state: SharedState, labels: Vec<String>) -> Response {
    if labels.is_empty() {
        return Response::json(AgentsWithLabelsResponse { handlers: vec![] });
    }

    let users_dir = state.repo_root.join("users");
    let active: Vec<String> = state.users.read().await.clone();

    let handlers =
        tokio::task::spawn_blocking(move || scan_active_for_labels(&users_dir, &active, labels))
            .await
            .unwrap_or_default();

    Response::json(AgentsWithLabelsResponse { handlers })
}

/// Best-effort scan used by `handle_create_card` to populate
/// `CreateCardResponse.suggested_assignees`. Returns empty if `card_labels`
/// is empty or if no agent matches; never panics on bad yaml (logs warn).
pub async fn compute_suggested_assignees(
    state: &SharedState,
    card_labels: Vec<String>,
) -> Vec<String> {
    if card_labels.is_empty() {
        return vec![];
    }
    let users_dir = state.repo_root.join("users");
    let active: Vec<String> = state.users.read().await.clone();

    tokio::task::spawn_blocking(move || scan_active_for_labels(&users_dir, &active, card_labels))
        .await
        .unwrap_or_default()
}

/// Shared scan: return `active` handlers whose user.meta.yaml.labels ⊇ query.
/// Sorts result, dedupes (BTreeSet collection).
fn scan_active_for_labels(
    users_dir: &std::path::Path,
    active: &[String],
    query: Vec<String>,
) -> Vec<String> {
    let query_set: BTreeSet<String> = query.into_iter().collect();
    let mut matched: BTreeSet<String> = BTreeSet::new();
    for handler in active {
        let path = users_dir.join(format!("{}.meta.yaml", handler));
        let yaml = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(_) => continue, // missing/unreadable: skip, don't fail the whole scan
        };
        let meta: UserMeta = match serde_yaml::from_str(&yaml) {
            Ok(m) => m,
            Err(e) => {
                warn!("scan_active_for_labels: skip @{handler} (parse error: {e})");
                continue;
            }
        };
        let agent_set: BTreeSet<String> = meta.labels.into_iter().collect();
        if query_set.is_subset(&agent_set) {
            matched.insert(handler.clone());
        }
    }
    matched.into_iter().collect()
}

/// `caller_handler == target` check. The caller's identity is the daemon's
/// own bound handler (per-clone daemon model — every IPC into this daemon
/// is implicitly authored by `state.current_user`). See requirements P4
/// "Enforcement 机制".
async fn ensure_self(state: &SharedState, target: &str) -> Result<(), Response> {
    let me = state.current_user.read().await.clone().unwrap_or_default();
    if me != target {
        return Err(Response::error_with_code(
            format!("only self ({}) can modify own labels", me),
            "not_self",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    //! Unit tests for `scan_active_for_labels` — the pure scanning logic
    //! used by both `handle_agents_with_labels` and
    //! `compute_suggested_assignees`.
    //!
    //! Full handler integration (with commit_lock + git_storage) is covered
    //! by the daemon integration test harness; here we exercise the
    //! filesystem-scan logic in isolation.

    use super::*;

    fn write_user(dir: &std::path::Path, handler: &str, labels: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        let labels_yaml = if labels.is_empty() {
            "labels: []\n".to_string()
        } else {
            let lines: Vec<String> = labels.iter().map(|l| format!("  - {l}")).collect();
            format!("labels:\n{}\n", lines.join("\n"))
        };
        let yaml = format!(
            "display_name: {h}\nrole: member\nintroduction: \"\"\n{labels}",
            h = handler,
            labels = labels_yaml,
        );
        std::fs::write(dir.join(format!("{handler}.meta.yaml")), yaml).unwrap();
    }

    #[test]
    fn scan_empty_query_returns_all_active() {
        // NOTE: `scan_active_for_labels` does NOT special-case empty queries
        // because the empty set is a subset of any set (so all active users
        // "match"). The empty-query → empty-result policy lives in
        // `handle_agents_with_labels` (early return) and
        // `compute_suggested_assignees` (early return), not in this helper.
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &["rust"]);
        write_user(tmp.path(), "bob", &["python"]);
        let active: Vec<String> = vec!["alice".into(), "bob".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec![]);
        assert_eq!(got, vec!["alice", "bob"]);
    }

    #[test]
    fn scan_all_of_match() {
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &["rust", "backend"]);
        write_user(tmp.path(), "bob", &["rust", "frontend"]);
        write_user(tmp.path(), "carol", &["python"]);
        let active: Vec<String> = vec!["alice".into(), "bob".into(), "carol".into()];

        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        assert_eq!(got, vec!["alice", "bob"]);

        let got =
            scan_active_for_labels(tmp.path(), &active, vec!["rust".into(), "backend".into()]);
        assert_eq!(got, vec!["alice"]);

        let got = scan_active_for_labels(tmp.path(), &active, vec!["python".into()]);
        assert_eq!(got, vec!["carol"]);

        let got = scan_active_for_labels(tmp.path(), &active, vec!["go".into()]);
        assert!(got.is_empty());
    }

    #[test]
    fn scan_skips_archived_handlers() {
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &["rust"]);
        // bob's yaml exists but bob is NOT in `active` list (simulating depart)
        write_user(tmp.path(), "bob", &["rust"]);
        let active: Vec<String> = vec!["alice".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        assert_eq!(got, vec!["alice"]);
    }

    #[test]
    fn scan_skips_missing_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &["rust"]);
        // bob is in active but yaml file doesn't exist
        let active: Vec<String> = vec!["alice".into(), "bob".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        assert_eq!(got, vec!["alice"]);
    }

    #[test]
    fn scan_skips_malformed_yaml() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path()).unwrap();
        std::fs::write(tmp.path().join("alice.meta.yaml"), "not: valid: yaml::").unwrap();
        write_user(tmp.path(), "bob", &["rust"]);
        let active: Vec<String> = vec!["alice".into(), "bob".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        // alice's malformed yaml is skipped silently (log warn); bob still matches
        assert_eq!(got, vec!["bob"]);
    }

    #[test]
    fn scan_returns_sorted_dedupe() {
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &["rust"]);
        write_user(tmp.path(), "bob", &["rust"]);
        write_user(tmp.path(), "carol", &["rust"]);
        // active passed in non-alphabetical order; result still sorted
        let active: Vec<String> = vec!["carol".into(), "alice".into(), "bob".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        assert_eq!(got, vec!["alice", "bob", "carol"]);
    }

    #[test]
    fn scan_user_with_no_labels_does_not_match() {
        let tmp = tempfile::tempdir().unwrap();
        write_user(tmp.path(), "alice", &[]);
        let active: Vec<String> = vec!["alice".into()];
        let got = scan_active_for_labels(tmp.path(), &active, vec!["rust".into()]);
        assert!(got.is_empty());
    }
}
