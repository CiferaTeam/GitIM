#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! HTTP integration tests for the runtime's workspace lifecycle routes
//! (`GET /workspaces`, `POST /workspaces`, `GET /workspaces/{slug}`,
//! `DELETE /workspaces/{slug}`) plus the `WorkspaceSlug` extractor applied to
//! the nested `/workspaces/{slug}/im/...` namespace.
//!
//! ## Design
//!
//! We use `tower::ServiceExt::oneshot` to dispatch single requests through the
//! real axum router — no TCP listener, no spawned server, no port races. This
//! mirrors the pattern established in `tests/runtime_http.rs`.
//!
//! ## Why we inject state directly for happy-path "create" assertions
//!
//! `POST /workspaces` with `provider=local` would reach
//! `provision_local_workspace` → `git init --bare` → `provision_human`, which
//! spawns a real `gitim-daemon` process. Integration coverage for that deep
//! path lives in `tests/git_init_local.rs`.
//!
//! For slug normalization, listing, and delete semantics we want the *HTTP
//! surface* behaviour without the daemon tax. So for tests that assert
//! post-create state (tests 3–6, 10, 12, 13) we acquire the shared state via
//! `create_router`'s return tuple and inject a `WorkspaceContext` into
//! `state.workspaces` before hitting the relevant GET/DELETE routes. The slug
//! produced by a real POST would have gone through the same
//! `slug::normalize` + `slug::resolve` pair we invoke here, so the observable
//! semantics are preserved without starting a daemon.
//!
//! Tests 7 and 8 exercise genuine POST-side early-fail branches (missing
//! field → 4xx from serde; unresolvable workspace path → 400 with
//! `invalid_workspace_path`).
//!
//! ## HOME isolation
//!
//! `workspaces_create` and `workspaces_delete` persist to
//! `$HOME/.gitim/runtime.json`. Every test that can mutate HOME-resident
//! state sets `HOME` to a fresh `tempfile::TempDir` before building the
//! router and restores it on drop. Because `std::env::set_var` is
//! process-global, these tests are serialised via
//! `#[serial(http_workspaces_home)]`.

use std::collections::HashSet;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{json, Value};
use serial_test::serial;
use tempfile::TempDir;
use tower::ServiceExt;

#[cfg(feature = "test-support")]
use gitim_runtime::assets::AssetSource;
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::slug;
use gitim_runtime::workspace::WorkspaceContext;
#[cfg(feature = "test-support")]
use gitim_runtime::workspace::WorkspaceInitialization;

mod common;
use common::HomeGuard;
#[cfg(feature = "test-support")]
use common::{ensure_daemon_in_path, short_tempdir};

// -- Request helpers --------------------------------------------------------

/// Send a one-shot request through the router and decode the JSON body.
///
/// Returns `(status, json)`. A non-JSON body surfaces as `Value::Null` rather
/// than a panic — error-branch responses all carry JSON today but we stay
/// lenient so test failures read as status-code mismatches, not JSON panics.
async fn send(
    router: axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let builder = Request::builder().method(method).uri(uri);
    let req = match body {
        Some(b) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&b).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

/// Inject a pre-provisioned WorkspaceContext directly into the shared state.
///
/// Mirrors what a successful `POST /workspaces` would leave in state after
/// `provision_*_workspace` returns (slug in `workspaces`, `git_config` set,
/// `human_repo` set). Used by tests that need observable post-create state
/// without spawning a daemon.
fn inject_workspace(
    state: &SharedRuntimeState,
    slug_str: &str,
    workspace_name: &str,
    path: &std::path::Path,
    provider: GitProvider,
) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut ctx = WorkspaceContext::new(
        slug_str.to_string(),
        workspace_name.to_string(),
        path.clone(),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: path.to_string_lossy().into_owned(),
        created_at: "2026-04-18T00:00:00Z".to_string(),
        git: GitConfig {
            provider,
            remote_url: None,
            token: None,
            github_email: None,
        },
    });
    // `human_repo: None` keeps `initialized` false, matching a placeholder
    // that hasn't been fully onboarded yet. Tests 10/12/13 use this path.
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug_str.to_string(), ctx);
}

fn inject_github_workspace_with_remote(
    state: &SharedRuntimeState,
    slug_str: &str,
    workspace_name: &str,
    path: &std::path::Path,
    remote_url: &str,
) {
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let mut ctx = WorkspaceContext::new(
        slug_str.to_string(),
        workspace_name.to_string(),
        path.clone(),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: path.to_string_lossy().into_owned(),
        created_at: "2026-04-18T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some(remote_url.to_string()),
            token: Some("tok".to_string()),
            github_email: None,
        },
    });
    state
        .lock()
        .unwrap()
        .workspaces
        .insert(slug_str.to_string(), ctx);
}

// -- 1. list empty ----------------------------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn list_workspaces_empty() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["workspaces"], json!([]));
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn list_workspaces_includes_remote_identity_for_github_only() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();
    let root = TempDir::new().unwrap();

    inject_workspace(
        &state,
        "local-room",
        "Local Room",
        &root.path().join("local-room"),
        GitProvider::Local,
    );
    inject_github_workspace_with_remote(
        &state,
        "github-room",
        "GitHub Room",
        &root.path().join("github-room"),
        "https://github.com/Org/Repo.git",
    );

    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let workspaces = body["workspaces"].as_array().expect("workspaces");
    let local = workspaces
        .iter()
        .find(|w| w["slug"] == "local-room")
        .expect("local workspace");
    let github = workspaces
        .iter()
        .find(|w| w["slug"] == "github-room")
        .expect("github workspace");

    assert!(local.get("remote_identity").is_none() || local["remote_identity"].is_null());
    assert_eq!(github["remote_identity"], "github.com/org/repo");
}

// -- 2. health shape --------------------------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn health_without_workspaces() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "GET", "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["service"], "gitim-runtime");
    assert_eq!(body["workspaces_count"], 0);
}

// -- 3. create happy-path (slug derivation + listing round-trip) -----------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_local_mode() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // A fully daemon-free stand-in for `POST /workspaces`. The slug the POST
    // route would produce is exactly `slug::normalize(basename)`, which we
    // compute and inject directly. Listing after this must show the entry.
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("project-frontend");
    std::fs::create_dir(&ws_path).unwrap();
    let basename = ws_path.file_name().unwrap().to_string_lossy().into_owned();
    let expected_slug = slug::normalize(&basename);
    assert_eq!(expected_slug, "project-frontend");

    inject_workspace(
        &state,
        &expected_slug,
        &basename,
        &ws_path,
        GitProvider::Local,
    );

    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body["workspaces"].as_array().expect("workspaces array");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["slug"], "project-frontend");
    assert_eq!(entries[0]["workspace_name"], "project-frontend");
    assert_eq!(entries[0]["provider"], "local");
}

// -- 4. slug conflict gets -2 suffix ---------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_slug_conflict_appends_suffix() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Two distinct parent tempdirs, both with a child named "frontend" — the
    // exact scenario the suffix rule exists to handle. We compute both slugs
    // via the same `slug::resolve` the POST route would use.
    let parent_a = TempDir::new().unwrap();
    let parent_b = TempDir::new().unwrap();
    let ws_a = parent_a.path().join("frontend");
    let ws_b = parent_b.path().join("frontend");
    std::fs::create_dir(&ws_a).unwrap();
    std::fs::create_dir(&ws_b).unwrap();

    let candidate = slug::normalize("frontend");
    let slug_a = slug::resolve(&candidate, &HashSet::new());
    let mut existing: HashSet<String> = HashSet::new();
    existing.insert(slug_a.clone());
    let slug_b = slug::resolve(&candidate, &existing);

    assert_eq!(slug_a, "frontend");
    assert_eq!(slug_b, "frontend-2");

    inject_workspace(&state, &slug_a, "frontend", &ws_a, GitProvider::Local);
    inject_workspace(&state, &slug_b, "frontend", &ws_b, GitProvider::Local);

    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let slugs: Vec<String> = body["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["slug"].as_str().unwrap().to_string())
        .collect();
    // `workspaces_list` sorts alphabetically — "frontend" before "frontend-2".
    assert_eq!(slugs, vec!["frontend", "frontend-2"]);
}

// -- 5. unicode basename → "workspace" fallback ----------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_normalizes_unicode_basename() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Non-ASCII chars are stripped by `slug::normalize` which then trims and
    // falls back to "workspace" when the result is empty. A real POST with
    // basename "前端" would write this exact slug.
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("前端");
    std::fs::create_dir(&ws_path).unwrap();
    let basename = ws_path.file_name().unwrap().to_string_lossy().into_owned();
    let expected_slug = slug::normalize(&basename);
    assert_eq!(expected_slug, "workspace");

    inject_workspace(
        &state,
        &expected_slug,
        &basename,
        &ws_path,
        GitProvider::Local,
    );

    let (status, body) = send(router, "GET", "/workspaces/workspace", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "workspace");
    // workspace_name preserves the original (unicode) basename — the HTTP
    // response echoes the human-friendly label even when the slug can't.
    assert_eq!(body["workspace_name"], "前端");
}

// -- 6. uppercase basename → lowercase slug --------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_normalizes_uppercase() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("Frontend");
    std::fs::create_dir(&ws_path).unwrap();
    let basename = ws_path.file_name().unwrap().to_string_lossy().into_owned();
    let expected_slug = slug::normalize(&basename);
    assert_eq!(expected_slug, "frontend");

    inject_workspace(
        &state,
        &expected_slug,
        &basename,
        &ws_path,
        GitProvider::Local,
    );

    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let entries = body["workspaces"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0]["slug"], "frontend");
    // Original case preserved in the display name.
    assert_eq!(entries[0]["workspace_name"], "Frontend");
}

// -- 7. missing "path" field → 4xx -----------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_missing_path() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();

    // Deliberately omit `path`. Axum's Json extractor rejects this before the
    // handler runs — accept any 4xx since the exact code (400 vs 422) depends
    // on the axum version.
    let (status, _body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({ "git": { "provider": "local" } })),
    )
    .await;
    assert!(
        status.is_client_error(),
        "expected 4xx for missing path, got {status}"
    );
}

// -- 8. unresolvable workspace path → 400 ---------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_nonexistent_path() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();

    // Workspace identity is canonicalized before state reservation or
    // provisioning. A path below a regular file therefore fails without
    // creating any workspace-owned artifacts.
    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": "/dev/null/nonexistent-workspace-path",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert!(
        body.get("error_code").is_some(),
        "expected error_code on 400 body, got {body}"
    );
    assert!(
        body["error"].is_string() && !body["error"].as_str().unwrap().is_empty(),
        "expected non-empty error string, got {body}"
    );
}

// -- 9. unknown slug → 404 on GET ------------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn get_workspace_invalid_slug_returns_400() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "GET", "/workspaces/UPPER", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("invalid slug"),
        "expected invalid-slug error, got: {err}"
    );
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn get_workspace_unknown_returns_404() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "GET", "/workspaces/nonexistent", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "unknown workspace");
}

// -- 10. GET happy path ----------------------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn get_workspace_happy_path() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("backend");
    std::fs::create_dir(&ws_path).unwrap();
    inject_workspace(&state, "backend", "backend", &ws_path, GitProvider::Local);

    let (status, body) = send(router, "GET", "/workspaces/backend", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["slug"], "backend");
    assert_eq!(body["workspace_name"], "backend");
    assert_eq!(
        body["path"],
        ws_path.canonicalize().unwrap().to_string_lossy().as_ref()
    );
    assert_eq!(body["provider"], "local");
    assert_eq!(body["agents_count"], 0);
    // `initialized` is false because we injected without `human_repo`.
    assert_eq!(body["initialized"], false);
}

// -- 11. DELETE unknown → 404 ---------------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn delete_workspace_invalid_slug_returns_400() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "DELETE", "/workspaces/UPPER", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("invalid slug"),
        "expected invalid-slug error, got: {err}"
    );
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn delete_workspace_unknown_returns_404() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let (status, body) = send(router, "DELETE", "/workspaces/nonexistent", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "unknown workspace");
}

// -- 12. DELETE removes entry from listing --------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn delete_workspace_removes_entry() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("to-remove");
    std::fs::create_dir(&ws_path).unwrap();
    inject_workspace(
        &state,
        "to-remove",
        "to-remove",
        &ws_path,
        GitProvider::Local,
    );

    // Confirm it's listed first so we can tell DELETE is what removed it.
    let (status, body) = send(router.clone(), "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    let slugs_before: Vec<String> = body["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["slug"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(slugs_before, vec!["to-remove"]);

    let (status, body) = send(router.clone(), "DELETE", "/workspaces/to-remove", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], true);

    let (status, body) = send(router, "GET", "/workspaces", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["workspaces"], json!([]));
}

// -- 13. DELETE preserves local files -------------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn delete_workspace_preserves_local_files() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Stage a sentinel file inside the workspace directory. DELETE should
    // only clean up runtime artifacts (daemons, config) — never user files.
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("keep-files");
    std::fs::create_dir(&ws_path).unwrap();
    let sentinel = ws_path.join("hello.txt");
    std::fs::write(&sentinel, b"user data").unwrap();

    inject_workspace(
        &state,
        "keep-files",
        "keep-files",
        &ws_path,
        GitProvider::Local,
    );

    let (status, _body) = send(router, "DELETE", "/workspaces/keep-files", None).await;
    assert_eq!(status, StatusCode::OK);

    assert!(ws_path.exists(), "workspace dir should still exist");
    assert!(sentinel.exists(), "user file should still exist");
    assert_eq!(
        std::fs::read(&sentinel).unwrap(),
        b"user data",
        "user file contents should be untouched",
    );
}

// -- 14. invalid slug on workspace-scoped route → 400 ---------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn workspace_scoped_route_invalid_slug_returns_400() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    // Uppercase is rejected by `slug::validate` (runs in `WorkspaceSlug`
    // extractor) — no state lookup happens.
    let (status, body) = send(router, "GET", "/workspaces/UPPER/im/channels", None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    let err = body["error"].as_str().unwrap_or("");
    assert!(
        err.contains("invalid slug"),
        "expected invalid-slug error, got: {err}"
    );
}

// -- 15. unknown slug on workspace-scoped route → 404 ---------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn workspace_scoped_route_unknown_slug_returns_404() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    // Slug passes validation but no workspace with this slug exists, so the
    // downstream `human_client` helper returns the `unknown workspace` 404.
    let (status, body) = send(router, "GET", "/workspaces/nonexistent/im/channels", None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error"], "unknown workspace");
}

// -- 16. POST rejects already-registered path ------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_invalid_explicit_slug() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("explicit-slug-test");
    std::fs::create_dir(&ws_path).unwrap();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "slug": "Not Valid!",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "invalid_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_leading_hyphen_slug() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("leading-hyphen-test");
    std::fs::create_dir(&ws_path).unwrap();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "slug": "-leading",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "invalid_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_trailing_hyphen_slug() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("trailing-hyphen-test");
    std::fs::create_dir(&ws_path).unwrap();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "slug": "trailing-",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "invalid_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_consecutive_hyphens_slug() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("consecutive-hyphens-test");
    std::fs::create_dir(&ws_path).unwrap();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "slug": "foo--bar",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "invalid_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_reserved_slug() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("reserved-slug-test");
    std::fs::create_dir(&ws_path).unwrap();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "slug": "default",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "reserved_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_reserved_slug_conflict() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Seed an existing workspace.
    let parent = TempDir::new().unwrap();
    let existing_path = parent.path().join("existing");
    std::fs::create_dir(&existing_path).unwrap();
    inject_workspace(
        &state,
        "default-2",
        "Existing",
        &existing_path,
        GitProvider::Local,
    );

    // A POST with explicit slug "default" must fail with reserved_slug
    // because "default" is reserved and would resolve to "default-2",
    // which is already taken.
    let other_parent = TempDir::new().unwrap();
    let other_path = other_parent.path().join("other");
    std::fs::create_dir(&other_path).unwrap();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": other_path.to_string_lossy(),
            "slug": "default",
            "git": { "provider": "local" },
        })),
    )
    .await;
    // validate() catches reserved slug before resolve(), so it's a 400
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "reserved_slug");
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_duplicate_explicit_slug() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Seed an existing workspace.
    let parent = TempDir::new().unwrap();
    let existing_path = parent.path().join("existing");
    std::fs::create_dir(&existing_path).unwrap();
    inject_workspace(
        &state,
        "taken-slug",
        "Existing",
        &existing_path,
        GitProvider::Local,
    );

    // A second POST with a different path but the same explicit slug must fail
    // before provisioning, without mutating the existing workspace.
    let other_parent = TempDir::new().unwrap();
    let other_path = other_parent.path().join("other");
    std::fs::create_dir(&other_path).unwrap();
    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": other_path.to_string_lossy(),
            "slug": "taken-slug",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "slug_conflict");

    let s = state.lock().unwrap();
    assert_eq!(s.workspaces.len(), 1);
    assert!(s.workspaces.contains_key("taken-slug"));
}

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_duplicate_path() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Seed with an existing workspace at a concrete path.
    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("first-workspace");
    std::fs::create_dir(&ws_path).unwrap();
    inject_workspace(
        &state,
        "first-workspace",
        "first-workspace",
        &ws_path,
        GitProvider::Local,
    );

    // Second POST with the same path must fail with `workspace_path_exists`
    // BEFORE any provisioning or slug allocation. The existing workspace's
    // daemon + .gitim-runtime/ stay untouched.
    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": ws_path.to_string_lossy(),
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["ok"], false);
    assert_eq!(body["error_code"], "workspace_path_exists");
    assert_eq!(body["existing_slug"], "first-workspace");

    // The seeded workspace is still in state — the duplicate attempt didn't
    // mutate anything.
    let s = state.lock().unwrap();
    assert_eq!(s.workspaces.len(), 1);
    assert!(s.workspaces.contains_key("first-workspace"));
}

#[cfg(unix)]
#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_a_leaf_symlink_alias_before_provider_side_effects() {
    use std::os::unix::fs::symlink;

    let _home = HomeGuard::install();
    let (router, state) = create_router();
    let parent = TempDir::new().unwrap();
    let workspace = parent.path().join("canonical-room");
    let alias = parent.path().join("room-alias");
    std::fs::create_dir(&workspace).unwrap();
    symlink(&workspace, &alias).unwrap();
    let sentinel = workspace.join(".gitim-runtime/sentinel");
    std::fs::create_dir_all(sentinel.parent().unwrap()).unwrap();
    std::fs::write(&sentinel, b"active-workspace").unwrap();
    inject_workspace(
        &state,
        "canonical-room",
        "Canonical Room",
        &workspace,
        GitProvider::Local,
    );
    let token = state.lock().unwrap().workspaces["canonical-room"]
        .asset_token
        .clone();

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": alias,
            "slug": "alias-room",
            "git": { "provider": "github" },
        })),
    )
    .await;

    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["error_code"], "workspace_path_exists");
    assert_eq!(body["existing_slug"], "canonical-room");
    let runtime = state.lock().unwrap();
    assert_eq!(runtime.workspaces.len(), 1);
    assert_eq!(runtime.workspaces["canonical-room"].asset_token, token);
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"active-workspace");
}

#[cfg(all(unix, feature = "test-support"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn all_workspace_alias_forms_leave_an_active_workspace_byte_for_byte_unchanged() {
    use std::os::unix::fs::symlink;

    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let root = short_tempdir();
    let real_parent = root.path().join("real-parent");
    let workspace = real_parent.join("room");
    let nested = real_parent.join("nested");
    std::fs::create_dir_all(&workspace).unwrap();
    std::fs::create_dir(&nested).unwrap();
    let leaf_alias = root.path().join("room-link");
    let parent_alias = root.path().join("parent-link");
    symlink(&workspace, &leaf_alias).unwrap();
    symlink(&real_parent, &parent_alias).unwrap();
    let lexical_alias = nested.join("..").join("room");
    let canonical_workspace = workspace.canonicalize().unwrap();
    let (router, state) = create_router();

    let (status, _) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical_workspace,
            "slug": "room",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (assets, token, binding) = {
        let runtime = state.lock().unwrap();
        let context = &runtime.workspaces["room"];
        let config = context.git_config.as_ref().unwrap();
        (
            std::sync::Arc::clone(&runtime.assets),
            context.asset_token.clone(),
            format!("local:{}", config.created_at),
        )
    };
    let store = assets
        .open_registered_store(&canonical_workspace, &binding, &token)
        .unwrap();
    let stored = store
        .put_bytes(b"alias-safety-object", AssetSource::LocalUpload)
        .unwrap();
    let object_path = canonical_workspace
        .join(".gitim-runtime/assets/v1/objects/sha256")
        .join(&stored.sha256[..2])
        .join(&stored.sha256);
    let runtime_json =
        std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".gitim/runtime.json");
    let workspace_config = canonical_workspace.join(".gitim-runtime/config.json");
    let manifest = canonical_workspace.join(".gitim-runtime/assets/v1/store.json");
    let pid_file = canonical_workspace.join(".gitim-runtime/human/.gitim/run/gitim.pid");
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
    let baseline_runtime_json = std::fs::read(&runtime_json).unwrap();
    let baseline_workspace_config = std::fs::read(&workspace_config).unwrap();
    let baseline_manifest = std::fs::read(&manifest).unwrap();
    let baseline_object = std::fs::read(&object_path).unwrap();
    let baseline_usage = store.usage().unwrap();
    let baseline_health = assets.health_snapshot(&canonical_workspace).unwrap();
    let orphaned = canonical_workspace.join(".gitim-runtime/orphaned-assets");
    let baseline_quarantine = std::fs::read_dir(&orphaned)
        .map(|entries| entries.count())
        .unwrap_or(0);

    let aliases = [
        (leaf_alias, "leaf-alias", json!({ "provider": "local" })),
        (
            parent_alias.join("room"),
            "parent-alias",
            json!({
                "provider": "github",
                "remote_url": "https://github.com/example/never-contact",
                "token": "must-not-be-used"
            }),
        ),
        (
            lexical_alias,
            "lexical-alias",
            json!({ "provider": "local" }),
        ),
    ];
    for (alias, slug, git) in aliases {
        let (status, body) = send(
            router.clone(),
            "POST",
            "/workspaces",
            Some(json!({ "path": alias, "slug": slug, "git": git })),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{slug}");
        assert_eq!(body["error_code"], "workspace_path_exists", "{slug}");
        assert_eq!(body["existing_slug"], "room", "{slug}");
        assert!(std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .unwrap()
            .success());
        assert_eq!(std::fs::read(&runtime_json).unwrap(), baseline_runtime_json);
        assert_eq!(
            std::fs::read(&workspace_config).unwrap(),
            baseline_workspace_config
        );
        assert_eq!(std::fs::read(&manifest).unwrap(), baseline_manifest);
        assert_eq!(std::fs::read(&object_path).unwrap(), baseline_object);
        assert_eq!(store.usage().unwrap(), baseline_usage);
        assert_eq!(
            assets.health_snapshot(&canonical_workspace).unwrap(),
            baseline_health
        );
        assert_eq!(
            std::fs::read_dir(&orphaned)
                .map(|entries| entries.count())
                .unwrap_or(0),
            baseline_quarantine
        );
        let runtime = state.lock().unwrap();
        assert_eq!(runtime.workspaces.len(), 1);
        assert_eq!(runtime.workspaces["room"].asset_token, token);
        assert_eq!(assets.lifecycle_registry_entries(), (1, 1));
    }

    assert_eq!(
        send(router, "DELETE", "/workspaces/room", None).await.0,
        StatusCode::OK
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn deleted_local_workspace_assets_survive_failed_and_successful_reregistration() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("asset-reregister");
    std::fs::create_dir(&workspace).unwrap();
    let canonical = workspace.canonicalize().unwrap();
    let (router, state) = create_router();

    let (status, body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "asset-reregister",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let (assets, token, original_created_at) = {
        let runtime = state.lock().unwrap();
        let context = &runtime.workspaces["asset-reregister"];
        (
            std::sync::Arc::clone(&runtime.assets),
            context.asset_token.clone(),
            context.git_config.as_ref().unwrap().created_at.clone(),
        )
    };
    let store = assets
        .open_registered_store(&canonical, &format!("local:{original_created_at}"), &token)
        .unwrap();
    let stored = store
        .put_bytes(b"preserved-attachment", AssetSource::LocalUpload)
        .unwrap();
    let object_path = store.object_path(&stored.sha256).unwrap();
    let metadata_path = store.metadata_path(&stored.sha256).unwrap();

    assert_eq!(
        send(
            router.clone(),
            "DELETE",
            "/workspaces/asset-reregister",
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    let (failed_status, failed_body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "asset-reregister",
            "git": {
                "provider": "github",
                "remote_url": "https://github.com/acme/room"
            },
        })),
    )
    .await;
    assert_eq!(failed_status, StatusCode::BAD_REQUEST);
    assert_eq!(failed_body["error_code"], "missing_token");
    assert!(object_path.exists());
    assert!(metadata_path.exists());
    assert_eq!(
        WorkspaceConfig::read(&canonical).unwrap().created_at,
        original_created_at
    );

    let (recreated_status, recreated_body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "asset-reregister",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(recreated_status, StatusCode::CREATED, "{recreated_body}");
    let (new_token, new_created_at) = {
        let runtime = state.lock().unwrap();
        let context = &runtime.workspaces["asset-reregister"];
        (
            context.asset_token.clone(),
            context.git_config.as_ref().unwrap().created_at.clone(),
        )
    };
    assert_eq!(new_created_at, original_created_at);
    let reopened = assets
        .open_registered_store(&canonical, &format!("local:{new_created_at}"), &new_token)
        .unwrap();
    assert_eq!(
        reopened.read(&stored.sha256).unwrap(),
        b"preserved-attachment"
    );
    assert!(!canonical.join(".gitim-runtime/orphaned-assets").exists());

    assert_eq!(
        send(router, "DELETE", "/workspaces/asset-reregister", None,)
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn failed_reregistration_restores_valid_config_without_existing_assets() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("config-reregister");
    std::fs::create_dir(&workspace).unwrap();
    let canonical = workspace.canonicalize().unwrap();
    let (router, _state) = create_router();

    let (status, body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "config-reregister",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let original_created_at = WorkspaceConfig::read(&canonical).unwrap().created_at;
    assert_eq!(
        send(
            router.clone(),
            "DELETE",
            "/workspaces/config-reregister",
            None,
        )
        .await
        .0,
        StatusCode::OK
    );
    std::fs::remove_dir_all(canonical.join(".gitim-runtime/assets")).unwrap();
    assert!(!canonical.join(".gitim-runtime/assets/v1").exists());

    let (failed_status, failed_body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "config-reregister",
            "git": {
                "provider": "github",
                "remote_url": "https://github.com/acme/room"
            },
        })),
    )
    .await;
    assert_eq!(failed_status, StatusCode::BAD_REQUEST);
    assert_eq!(failed_body["error_code"], "missing_token");
    assert_eq!(
        WorkspaceConfig::read(&canonical).unwrap().created_at,
        original_created_at
    );
    assert!(!canonical.join(".gitim-runtime/assets/v1").exists());

    let (recreated_status, recreated_body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "config-reregister",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(recreated_status, StatusCode::CREATED, "{recreated_body}");
    assert_eq!(
        WorkspaceConfig::read(&canonical).unwrap().created_at,
        original_created_at
    );
    assert_eq!(
        send(router, "DELETE", "/workspaces/config-reregister", None)
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(all(unix, feature = "test-support"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn concurrent_canonical_and_alias_creates_have_one_side_effecting_winner() {
    use std::os::unix::fs::symlink;

    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let root = short_tempdir();
    let workspace = root.path().join("room");
    let alias = root.path().join("room-link");
    std::fs::create_dir(&workspace).unwrap();
    symlink(&workspace, &alias).unwrap();
    let canonical_workspace = workspace.canonicalize().unwrap();
    let (router, state) = create_router();
    let reservation_barrier = std::sync::Arc::new(tokio::sync::Barrier::new(3));
    state.lock().unwrap().workspace_create_reservation_barrier =
        Some(std::sync::Arc::clone(&reservation_barrier));

    let canonical_create = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical_workspace,
            "slug": "canonical-room",
            "git": { "provider": "local" },
        })),
    ));
    let alias_create = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": alias,
            "slug": "alias-room",
            "git": { "provider": "local" },
        })),
    ));
    reservation_barrier.wait().await;
    let first = canonical_create.await.unwrap();
    let second = alias_create.await.unwrap();
    let mut results = [first, second];
    results.sort_by_key(|(status, _)| status.as_u16());
    assert_eq!(results[0].0, StatusCode::CREATED);
    assert_eq!(results[1].0, StatusCode::CONFLICT);
    assert_eq!(results[1].1["error_code"], "workspace_path_exists");
    let winner = results[0].1["slug"].as_str().unwrap().to_string();

    let (assets, pid) = {
        let runtime = state.lock().unwrap();
        assert_eq!(runtime.workspaces.len(), 1);
        let context = &runtime.workspaces[&winner];
        assert_eq!(context.path, workspace.canonicalize().unwrap());
        let pid = std::fs::read_to_string(
            context
                .path
                .join(".gitim-runtime/human/.gitim/run/gitim.pid"),
        )
        .unwrap()
        .trim()
        .parse::<u32>()
        .unwrap();
        (std::sync::Arc::clone(&runtime.assets), pid)
    };
    assert!(std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    assert_eq!(gitim_runtime::user_config::read().workspaces.len(), 1);
    assert_eq!(assets.lifecycle_registry_entries(), (1, 1));
    assert_eq!(
        std::fs::read_dir(workspace.join(".gitim-runtime/orphaned-assets"))
            .map(|entries| entries.count())
            .unwrap_or(0),
        0
    );
    assert_eq!(
        send(router, "DELETE", &format!("/workspaces/{winner}"), None)
            .await
            .0,
        StatusCode::OK
    );
}

// -- 17. invalid POST path does not reserve placeholder state --------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn invalid_workspace_path_never_reserves_placeholder() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // Canonical path validation precedes placeholder reservation, so this
    // request cannot leave a half-initialized `WorkspaceContext` visible to
    // later requests.
    let (status, _body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": "/dev/null/nonexistent-workspace-path",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let s = state.lock().unwrap();
    assert!(
        s.workspaces.is_empty(),
        "failed create left placeholder in state: {:?}",
        s.workspaces.keys().collect::<Vec<_>>()
    );
}

#[cfg(unix)]
#[tokio::test]
#[serial(http_workspaces_home)]
async fn recovery_canonicalizes_aliases_deduplicates_paths_and_preserves_other_workspaces() {
    use std::os::unix::fs::symlink;

    let _home = HomeGuard::install();
    let root = TempDir::new().unwrap();
    let room = root.path().join("room");
    let room_alias = root.path().join("room-alias");
    let other = root.path().join("other");
    std::fs::create_dir(&room).unwrap();
    std::fs::create_dir(&other).unwrap();
    symlink(&room, &room_alias).unwrap();
    let room_canonical = room.canonicalize().unwrap();
    let other_canonical = other.canonicalize().unwrap();
    WorkspaceConfig {
        workspace: room_alias.to_string_lossy().into_owned(),
        created_at: "2026-07-12T00:00:00Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    }
    .write(&room)
    .unwrap();
    WorkspaceConfig {
        workspace: other.to_string_lossy().into_owned(),
        created_at: "2026-07-12T00:00:01Z".to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    }
    .write(&other)
    .unwrap();
    gitim_runtime::user_config::write(&gitim_runtime::user_config::UserConfig {
        runtime_id: "24a6489c-762e-4461-9247-a824807a6080".to_string(),
        workspaces: vec![
            gitim_runtime::user_config::WorkspaceEntry {
                slug: "room".to_string(),
                workspace_name: "Room".to_string(),
                path: room_alias.to_string_lossy().into_owned(),
            },
            gitim_runtime::user_config::WorkspaceEntry {
                slug: "room-duplicate".to_string(),
                workspace_name: "Duplicate".to_string(),
                path: room.to_string_lossy().into_owned(),
            },
            gitim_runtime::user_config::WorkspaceEntry {
                slug: "other".to_string(),
                workspace_name: "Other".to_string(),
                path: other.to_string_lossy().into_owned(),
            },
        ],
        listen_port: Some(17777),
        fleet_nodes: Vec::new(),
    })
    .unwrap();
    let (_router, state) = create_router();

    gitim_runtime::http::recover_from_config(std::sync::Arc::clone(&state)).await;

    let runtime = state.lock().unwrap();
    assert_eq!(runtime.workspaces.len(), 2);
    assert_eq!(runtime.workspaces["room"].path, room_canonical);
    assert_eq!(runtime.workspaces["other"].path, other_canonical);
    assert!(!runtime.workspaces.contains_key("room-duplicate"));
    drop(runtime);
    let persisted = gitim_runtime::user_config::read();
    assert_eq!(persisted.runtime_id, "24a6489c-762e-4461-9247-a824807a6080");
    assert_eq!(persisted.listen_port, Some(17777));
    assert_eq!(persisted.workspaces.len(), 2);
    assert_eq!(
        persisted
            .workspaces
            .iter()
            .find(|entry| entry.slug == "room")
            .unwrap()
            .path,
        room_canonical.to_string_lossy()
    );
    assert_eq!(
        WorkspaceConfig::read(&room).unwrap().workspace,
        room_canonical.to_string_lossy()
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn cancelled_create_request_finishes_owned_asset_activation_and_durable_state() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("cancel-safe-create");
    std::fs::create_dir(&workspace).unwrap();
    let (router, state) = create_router();
    let reached = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let resume = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    state.lock().unwrap().assets.inject_after_activation_pause(
        std::sync::Arc::clone(&reached),
        std::sync::Arc::clone(&resume),
    );

    let request = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": workspace,
            "slug": "cancel-safe-create",
            "git": { "provider": "local" },
        })),
    ));
    reached.wait().await;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert!(state.lock().unwrap().workspaces["cancel-safe-create"]
        .initialization
        .is_some());

    resume.wait().await;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let finished = state
                .lock()
                .unwrap()
                .workspaces
                .get("cancel-safe-create")
                .is_some_and(|workspace| workspace.initialization.is_none());
            if finished {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let (path, token, assets) = {
        let runtime = state.lock().unwrap();
        let context = &runtime.workspaces["cancel-safe-create"];
        (
            context.path.clone(),
            context.asset_token.clone(),
            std::sync::Arc::clone(&runtime.assets),
        )
    };
    let config = WorkspaceConfig::read(&path).unwrap();
    let binding = format!("local:{}", config.created_at);
    assets
        .open_registered_store(&path, &binding, &token)
        .expect("cancelled request left active registered store");
    assert!(gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .any(|entry| entry.slug == "cancel-safe-create"));

    let (status, _) = send(router, "DELETE", "/workspaces/cancel-safe-create", None).await;
    assert_eq!(status, StatusCode::OK);
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn delete_waits_for_owned_workspace_finalization_before_deactivating_exact_token() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("delete-during-create");
    std::fs::create_dir(&workspace).unwrap();
    let (router, state) = create_router();
    let reached = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let resume = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    state.lock().unwrap().assets.inject_after_activation_pause(
        std::sync::Arc::clone(&reached),
        std::sync::Arc::clone(&resume),
    );
    let create = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": workspace,
            "slug": "delete-during-create",
            "git": { "provider": "local" },
        })),
    ));
    reached.wait().await;

    let mut delete = tokio::spawn(send(
        router,
        "DELETE",
        "/workspaces/delete-during-create",
        None,
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut delete)
            .await
            .is_err(),
        "delete returned while workspace finalization was paused"
    );
    resume.wait().await;

    assert_eq!(create.await.unwrap().0, StatusCode::CREATED);
    assert_eq!(delete.await.unwrap().0, StatusCode::OK);
    assert!(state.lock().unwrap().workspaces.is_empty());
    assert!(gitim_runtime::user_config::read().workspaces.is_empty());
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn cancelled_delete_keeps_the_workspace_registered_for_retry() {
    let _home = HomeGuard::install();
    let workspace = short_tempdir();
    let canonical = workspace.path().canonicalize().unwrap();
    let (router, state) = create_router();
    inject_workspace(
        &state,
        "cancel-delete",
        "Cancel Delete",
        &canonical,
        GitProvider::Local,
    );
    let (assets, token) = {
        let runtime = state.lock().unwrap();
        (
            std::sync::Arc::clone(&runtime.assets),
            runtime.workspaces["cancel-delete"].asset_token.clone(),
        )
    };
    let store = assets
        .activate_workspace(&canonical, "local:2026-04-18T00:00:00Z", &token)
        .unwrap();
    let staged = store
        .stage_bytes("pending.bin", b"pending-delete")
        .await
        .unwrap();
    let transition_reached = std::sync::Arc::new(std::sync::Barrier::new(2));
    let transition_resume = std::sync::Arc::new(std::sync::Barrier::new(2));
    store.inject_persistence_transition_pause(
        std::sync::Arc::clone(&transition_reached),
        std::sync::Arc::clone(&transition_resume),
    );
    let request = tokio::spawn(send(
        router.clone(),
        "DELETE",
        "/workspaces/cancel-delete",
        None,
    ));
    tokio::task::spawn_blocking(move || transition_reached.wait())
        .await
        .unwrap();
    request.abort();
    tokio::task::spawn_blocking(move || transition_resume.wait())
        .await
        .unwrap();
    assert!(request.await.unwrap_err().is_cancelled());

    assert_eq!(
        state.lock().unwrap().workspaces["cancel-delete"].asset_token,
        token
    );
    assets
        .open_registered_store(&canonical, "local:2026-04-18T00:00:00Z", &token)
        .unwrap();
    drop(staged);
    assert_eq!(
        send(router, "DELETE", "/workspaces/cancel-delete", None)
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn delete_waiting_for_old_initialization_does_not_remove_recreated_workspace() {
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let old_path = parent.path().join("old-workspace");
    let new_path = parent.path().join("new-workspace");
    std::fs::create_dir(&old_path).unwrap();
    std::fs::create_dir(&new_path).unwrap();
    let (router, state) = create_router();

    let initialization = std::sync::Arc::new(WorkspaceInitialization::new());
    let wait_started = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    initialization.inject_wait_started(std::sync::Arc::clone(&wait_started));
    let mut old = WorkspaceContext::new("replace-race".to_string(), "Old".to_string(), old_path);
    old.initialization = Some(std::sync::Arc::clone(&initialization));
    state
        .lock()
        .unwrap()
        .workspaces
        .insert("replace-race".to_string(), old);

    let delete = tokio::spawn(send(router, "DELETE", "/workspaces/replace-race", None));
    wait_started.wait().await;

    let new_token = {
        let mut runtime = state.lock().unwrap();
        runtime.workspaces.remove("replace-race").unwrap();
        let recreated = WorkspaceContext::new(
            "replace-race".to_string(),
            "New".to_string(),
            new_path.clone(),
        );
        let token = recreated.asset_token.clone();
        runtime
            .workspaces
            .insert("replace-race".to_string(), recreated);
        token
    };
    initialization.finish();

    assert_eq!(delete.await.unwrap().0, StatusCode::OK);
    let runtime = state.lock().unwrap();
    let recreated = &runtime.workspaces["replace-race"];
    assert_eq!(recreated.asset_token, new_token);
    assert_eq!(recreated.path, new_path);
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn cancelled_create_rolls_back_when_owned_finalizer_panics_after_config_write() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("panic-safe-create");
    std::fs::create_dir(&workspace).unwrap();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
    let activation_reached = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let activation_resume = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    assets.inject_after_activation_pause(
        std::sync::Arc::clone(&activation_reached),
        std::sync::Arc::clone(&activation_resume),
    );
    assets.set_after_config_write_hook(std::sync::Arc::new(|| {
        panic!("injected owned workspace finalizer panic");
    }));

    let request = tokio::spawn(send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": workspace,
            "slug": "panic-safe-create",
            "git": { "provider": "local" },
        })),
    ));
    activation_reached.wait().await;
    let initialization = state.lock().unwrap().workspaces["panic-safe-create"]
        .initialization
        .clone()
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    activation_resume.wait().await;

    tokio::time::timeout(std::time::Duration::from_secs(5), initialization.wait())
        .await
        .expect("owned workspace finalizer did not settle after panic");
    assert!(!state
        .lock()
        .unwrap()
        .workspaces
        .contains_key("panic-safe-create"));
    assert!(gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .all(|entry| entry.slug != "panic-safe-create"));
    assert!(!workspace.join(".gitim-runtime").exists());
    assert!(assets.health_snapshot(&workspace).is_none());
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn owned_workspace_finalizer_panic_returns_500_after_complete_rollback() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("panic-status-create");
    std::fs::create_dir(&workspace).unwrap();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
    assets.set_after_config_write_hook(std::sync::Arc::new(|| {
        panic!("injected owned workspace finalizer panic");
    }));

    let (status, body) = send(
        router,
        "POST",
        "/workspaces",
        Some(json!({
            "path": workspace,
            "slug": "panic-status-create",
            "git": { "provider": "local" },
        })),
    )
    .await;

    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(body["error_code"], "asset_store_failed");
    assert!(!state
        .lock()
        .unwrap()
        .workspaces
        .contains_key("panic-status-create"));
    assert!(gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .all(|entry| entry.slug != "panic-status-create"));
    assert!(!workspace.join(".gitim-runtime").exists());
    assert!(assets.health_snapshot(&workspace).is_none());
    assert_eq!(
        assets
            .store_failures
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn finalizer_rollback_waits_for_asset_leases_before_atomic_cleanup() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("lease-safe-rollback");
    std::fs::create_dir(&workspace).unwrap();
    let canonical = workspace.canonicalize().unwrap();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
    let activation_reached = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let activation_resume = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    assets.inject_after_activation_pause(
        std::sync::Arc::clone(&activation_reached),
        std::sync::Arc::clone(&activation_resume),
    );
    let deactivation_attempted = std::sync::Arc::new(std::sync::Barrier::new(2));
    assets.inject_deactivate_attempt(std::sync::Arc::clone(&deactivation_attempted));
    assets.set_after_config_write_hook(std::sync::Arc::new(|| {
        panic!("injected lease-safe finalizer panic");
    }));

    let mut request = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "lease-safe-rollback",
            "git": { "provider": "local" },
        })),
    ));
    activation_reached.wait().await;
    let (token, binding) = {
        let runtime = state.lock().unwrap();
        let context = &runtime.workspaces["lease-safe-rollback"];
        let config = context.git_config.as_ref().unwrap();
        (
            context.asset_token.clone(),
            format!("local:{}", config.created_at),
        )
    };
    let store = assets
        .open_registered_store(&canonical, &binding, &token)
        .unwrap();
    let staged = store
        .stage_bytes("rollback.bin", b"rollback-lease")
        .await
        .unwrap();
    activation_resume.wait().await;
    tokio::task::spawn_blocking(move || deactivation_attempted.wait())
        .await
        .unwrap();

    let early = tokio::time::timeout(std::time::Duration::from_secs(1), &mut request).await;
    let rollback_waited = early.is_err();
    let context_remained = state
        .lock()
        .unwrap()
        .workspaces
        .get("lease-safe-rollback")
        .is_some_and(|context| context.asset_token == token);
    let config_remained = gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .any(|entry| entry.slug == "lease-safe-rollback");
    drop(staged);
    let (status, _body) = match early {
        Ok(result) => result.unwrap(),
        Err(_) => request.await.unwrap(),
    };

    assert!(
        rollback_waited,
        "rollback completed while an asset lease was live"
    );
    assert!(
        context_remained,
        "rollback removed its reachable context before teardown"
    );
    assert!(
        config_remained,
        "rollback removed durable config before teardown"
    );
    assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
    assert!(!state
        .lock()
        .unwrap()
        .workspaces
        .contains_key("lease-safe-rollback"));
    assert!(gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .all(|entry| entry.slug != "lease-safe-rollback"));
    assert!(!canonical.join(".gitim-runtime").exists());
    assert!(assets.health_snapshot(&canonical).is_none());
    assert_eq!(assets.lifecycle_registry_entries(), (0, 0));

    let (recreate_status, recreate_body) = send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical,
            "slug": "lease-safe-rollback",
            "git": { "provider": "local" },
        })),
    )
    .await;
    assert_eq!(recreate_status, StatusCode::CREATED, "{recreate_body}");
    assert_eq!(
        send(router, "DELETE", "/workspaces/lease-safe-rollback", None,)
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn rollback_never_cleans_a_canonical_workspace_owned_by_a_different_token() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("replacement-owner");
    std::fs::create_dir(&workspace).unwrap();
    let canonical_workspace = workspace.canonicalize().unwrap();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
    let activation_reached = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    let activation_resume = std::sync::Arc::new(tokio::sync::Barrier::new(2));
    assets.inject_after_activation_pause(
        std::sync::Arc::clone(&activation_reached),
        std::sync::Arc::clone(&activation_resume),
    );
    assets.set_after_config_write_hook(std::sync::Arc::new(|| {
        panic!("injected finalizer panic after owner replacement");
    }));

    let create = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": canonical_workspace,
            "slug": "replacement-owner",
            "git": { "provider": "local" },
        })),
    ));
    activation_reached.wait().await;

    let (human_repo, git_config, transition, pid) = {
        let runtime = state.lock().unwrap();
        let old = &runtime.workspaces["replacement-owner"];
        let human_repo = old.human_repo.clone().unwrap();
        let pid = std::fs::read_to_string(human_repo.join(".gitim/run/gitim.pid"))
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap();
        (
            human_repo,
            old.git_config.clone(),
            std::sync::Arc::clone(&old.transition),
            pid,
        )
    };
    let mut replacement = WorkspaceContext::new(
        "replacement-owner".to_string(),
        "Replacement Owner".to_string(),
        canonical_workspace.clone(),
    );
    replacement.human_repo = Some(human_repo);
    replacement.git_config = git_config;
    replacement.transition = transition;
    let replacement_token = replacement.asset_token.clone();
    state
        .lock()
        .unwrap()
        .workspaces
        .insert("replacement-owner".to_string(), replacement);
    let sentinel = canonical_workspace.join(".gitim-runtime/replacement-owner.sentinel");
    std::fs::write(&sentinel, b"new-owner").unwrap();
    activation_resume.wait().await;

    assert_eq!(create.await.unwrap().0, StatusCode::INTERNAL_SERVER_ERROR);
    {
        let runtime = state.lock().unwrap();
        assert_eq!(
            runtime.workspaces["replacement-owner"].asset_token,
            replacement_token
        );
    }
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"new-owner");
    assert!(canonical_workspace.join(".gitim-runtime").is_dir());
    assert!(std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .unwrap()
        .success());
    assert!(gitim_runtime::user_config::read()
        .workspaces
        .iter()
        .any(|entry| entry.slug == "replacement-owner"));
    assert_eq!(
        send(router, "DELETE", "/workspaces/replacement-owner", None)
            .await
            .0,
        StatusCode::OK
    );
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn rollback_config_io_does_not_hold_runtime_state_or_leak_transition_slots() {
    let _home = HomeGuard::install();
    let parent = short_tempdir();
    let workspace = parent.path().join("rollback-config-pause");
    std::fs::create_dir(&workspace).unwrap();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
    let rollback_reached = std::sync::Arc::new(std::sync::Barrier::new(2));
    let rollback_resume = std::sync::Arc::new(std::sync::Barrier::new(2));
    assets.inject_rollback_config_pause(
        std::sync::Arc::clone(&rollback_reached),
        std::sync::Arc::clone(&rollback_resume),
    );

    let create = tokio::spawn(send(
        router.clone(),
        "POST",
        "/workspaces",
        Some(json!({
            "path": workspace,
            "slug": "rollback-config-pause",
            "git": { "provider": "unsupported" },
        })),
    ));
    tokio::task::spawn_blocking(move || rollback_reached.wait())
        .await
        .unwrap();

    let list = tokio::spawn(send(router, "GET", "/workspaces", None));
    let (list_status, _) = tokio::time::timeout(std::time::Duration::from_secs(1), list)
        .await
        .expect("workspace list waited for rollback config I/O")
        .unwrap();
    assert_eq!(list_status, StatusCode::OK);

    tokio::task::spawn_blocking(move || rollback_resume.wait())
        .await
        .unwrap();
    assert_eq!(create.await.unwrap().0, StatusCode::BAD_REQUEST);
    let runtime = state.lock().unwrap();
    assert!(runtime.workspaces.is_empty());
    assert_eq!(runtime.workspace_transitions.live_entries(), 0);
}

#[cfg(feature = "test-support")]
#[tokio::test]
#[serial(http_workspaces_home)]
async fn repeated_registered_open_and_delete_cycles_leave_both_registries_empty() {
    let _home = HomeGuard::install();
    let root = short_tempdir();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);

    for index in 0..32 {
        let slug = format!("cycle-{index}");
        let workspace = root.path().join(&slug);
        std::fs::create_dir(&workspace).unwrap();
        let canonical = workspace.canonicalize().unwrap();
        let transition = state.lock().unwrap().workspace_transitions.slot(&canonical);
        let mut context = WorkspaceContext::new(slug.clone(), slug.clone(), canonical.clone());
        context.transition = transition;
        assets
            .activate_workspace(&canonical, format!("local:{index}"), &context.asset_token)
            .unwrap();
        state
            .lock()
            .unwrap()
            .workspaces
            .insert(slug.clone(), context);

        assert_eq!(
            send(
                router.clone(),
                "DELETE",
                &format!("/workspaces/{slug}"),
                None
            )
            .await
            .0,
            StatusCode::OK
        );
        let runtime = state.lock().unwrap();
        assert!(runtime.workspaces.is_empty());
        assert_eq!(runtime.workspace_transitions.live_entries(), 0);
        drop(runtime);
        assert_eq!(assets.lifecycle_registry_entries(), (0, 0));
    }
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn registered_open_waiters_do_not_retain_dead_path_indexes_after_delete() {
    let _home = HomeGuard::install();
    let root = short_tempdir();
    let (router, state) = create_router();
    let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);

    for index in 0..16 {
        let slug = format!("waiter-cycle-{index}");
        let workspace = root.path().join(&slug);
        std::fs::create_dir(&workspace).unwrap();
        let canonical = workspace.canonicalize().unwrap();
        let binding = format!("local:{index}");
        let transition = state.lock().unwrap().workspace_transitions.slot(&canonical);
        let mut context = WorkspaceContext::new(slug.clone(), slug.clone(), canonical.clone());
        context.transition = transition;
        let token = context.asset_token.clone();
        let store = assets
            .activate_workspace(&canonical, binding.clone(), &token)
            .unwrap();
        state
            .lock()
            .unwrap()
            .workspaces
            .insert(slug.clone(), context);

        let transition_reached = std::sync::Arc::new(std::sync::Barrier::new(2));
        let transition_resume = std::sync::Arc::new(std::sync::Barrier::new(2));
        store.inject_persistence_transition_pause(
            std::sync::Arc::clone(&transition_reached),
            std::sync::Arc::clone(&transition_resume),
        );
        let delete_router = router.clone();
        let delete_uri = format!("/workspaces/{slug}");
        let delete =
            tokio::spawn(async move { send(delete_router, "DELETE", &delete_uri, None).await });
        tokio::task::spawn_blocking(move || transition_reached.wait())
            .await
            .unwrap();

        let waiter_attempted = std::sync::Arc::new(std::sync::Barrier::new(2));
        assets.inject_registered_open_wait_attempt(std::sync::Arc::clone(&waiter_attempted));
        let waiter_assets = std::sync::Arc::clone(&assets);
        let waiter_path = canonical.clone();
        let waiter_binding = binding.clone();
        let waiter_token = token.clone();
        let waiter = tokio::task::spawn_blocking(move || {
            waiter_assets.open_registered_store(&waiter_path, &waiter_binding, &waiter_token)
        });
        tokio::task::spawn_blocking(move || waiter_attempted.wait())
            .await
            .unwrap();

        tokio::task::spawn_blocking(move || transition_resume.wait())
            .await
            .unwrap();
        assert_eq!(delete.await.unwrap().0, StatusCode::OK);
        assert!(matches!(
            waiter.await.unwrap(),
            Err(gitim_runtime::assets::AssetError::StaleBinding)
        ));
        assert_eq!(assets.lifecycle_registry_entries(), (0, 0));
    }
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial(http_workspaces_home)]
async fn asset_activation_store_and_invariant_failures_preserve_status_and_fully_rollback() {
    ensure_daemon_in_path();
    let _home = HomeGuard::install();

    for (slug, expected_status, inject) in [
        (
            "activation-store-failure",
            StatusCode::INSUFFICIENT_STORAGE,
            0_u8,
        ),
        (
            "activation-invariant-failure",
            StatusCode::INTERNAL_SERVER_ERROR,
            1_u8,
        ),
    ] {
        let parent = short_tempdir();
        let workspace = parent.path().join(slug);
        std::fs::create_dir(&workspace).unwrap();
        let (router, state) = create_router();
        let assets = std::sync::Arc::clone(&state.lock().unwrap().assets);
        let events = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        assets.set_event_observer({
            let events = std::sync::Arc::clone(&events);
            std::sync::Arc::new(move |event| events.lock().unwrap().push(event))
        });
        if inject == 0 {
            assets.inject_activation_store_failure_once();
        } else {
            assets.inject_activation_invariant_failure_once();
        }

        let (status, body) = send(
            router,
            "POST",
            "/workspaces",
            Some(json!({
                "path": workspace,
                "slug": slug,
                "git": { "provider": "local" },
            })),
        )
        .await;
        assert_eq!(status, expected_status, "{slug}");
        assert_eq!(body["error_code"], "asset_store_failed", "{slug}");
        assert!(state.lock().unwrap().workspaces.is_empty(), "{slug}");
        assert!(!workspace.join(".gitim-runtime").exists(), "{slug}");
        assert!(assets.health_snapshot(&workspace).is_none(), "{slug}");
        assert!(gitim_runtime::user_config::read().workspaces.is_empty());
        assert_eq!(
            assets
                .store_failures
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "{slug}"
        );
        let events = events.lock().unwrap();
        assert_eq!(events.len(), 1, "{slug}");
        assert_eq!(events[0].event, "asset_store_failure", "{slug}");
        assert_eq!(events[0].workspace_slug, slug, "{slug}");
        assert_eq!(events[0].error_code, Some("asset_store_failed"), "{slug}");
        assert!(!format!("{:?}", events[0]).contains(workspace.to_string_lossy().as_ref()));
    }
}

// -- 18. DELETE aborts in-process agent loop handles -----------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn delete_workspace_aborts_agent_loop_handles() {
    use std::sync::Arc;
    use tokio::sync::Notify;

    let _home = HomeGuard::install();
    let (router, state) = create_router();

    let parent = TempDir::new().unwrap();
    let ws_path = parent.path().join("loop-test");
    std::fs::create_dir(&ws_path).unwrap();
    inject_workspace(
        &state,
        "loop-test",
        "loop-test",
        &ws_path,
        GitProvider::Local,
    );

    // Spawn a tokio task that runs until aborted, and hand its AbortHandle to
    // the injected agent's `loop_handle`. This stands in for a real
    // `start_agent_loop`-spawned task: what we care about is that DELETE flips
    // the abort bit.
    let notify = Arc::new(Notify::new());
    let notify_clone = notify.clone();
    let task = tokio::spawn(async move {
        notify_clone.notify_one();
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
    });
    let abort_handle = task.abort_handle();

    notify.notified().await;

    {
        let mut s = state.lock().unwrap();
        let ctx = s.workspaces.get_mut("loop-test").unwrap();
        let mut agent_info = gitim_runtime::http::AgentInfo {
            id: "a".into(),
            handler: "a".into(),
            display_name: "a".into(),
            status: "running".into(),
            last_activity: None,
            messages_processed: 0,
            repo_path: ws_path.join("a").to_string_lossy().into_owned(),
            provider: Some("claude".into()),
            model: None,
            effort: None,
            system_prompt: None,
            introduction: None,
            env: Default::default(),
            error_message: None,
            session_usage: None,
            llm_provider: None,
            llm_model: None,
            usage_summary: None,
            saturation_summary: None,
            is_working: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            loop_generation: 0,
            loop_starting: false,
            loop_handle: None,
        };
        agent_info.loop_handle = Some(abort_handle);
        ctx.agents.insert("a".into(), agent_info);
    }

    let (status, _) = send(router, "DELETE", "/workspaces/loop-test", None).await;
    assert_eq!(status, StatusCode::OK);

    // After DELETE, the spawned task must observe its abort flag. Awaiting the
    // JoinHandle yields `Err(JoinError::is_cancelled)` once the abort fires.
    // Give it a bounded wait so this test stays fast if the fix regresses.
    let result = tokio::time::timeout(std::time::Duration::from_secs(2), task).await;
    let join_result = result.expect("agent loop task was not aborted within 2s");
    assert!(
        join_result.is_err() && join_result.unwrap_err().is_cancelled(),
        "task should have been aborted",
    );
}
