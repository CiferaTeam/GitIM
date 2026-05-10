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
//! field → 4xx from serde; path that can't host `repo.git` → 400 with
//! `clone_failed`).
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

use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::slug;
use gitim_runtime::workspace::WorkspaceContext;

// -- RAII HOME guard --------------------------------------------------------

/// Swaps `HOME` to a fresh tempdir and restores the prior value on drop.
/// Pairs with `#[serial(http_workspaces_home)]` so only one guard is live at
/// a time — avoiding the multi-threaded `set_var` race.
struct HomeGuard {
    original: Option<std::ffi::OsString>,
    _tmp: TempDir,
}

impl HomeGuard {
    fn install() -> Self {
        let tmp = TempDir::new().expect("tempdir for HOME");
        let original = std::env::var_os("HOME");
        std::env::set_var("HOME", tmp.path());
        Self {
            original,
            _tmp: tmp,
        }
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.original.take() {
            Some(val) => std::env::set_var("HOME", val),
            None => std::env::remove_var("HOME"),
        }
    }
}

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
    let mut ctx = WorkspaceContext::new(
        slug_str.to_string(),
        workspace_name.to_string(),
        path.to_path_buf(),
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

// -- 8. nonexistent/unwritable path → 400 ----------------------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_rejects_nonexistent_path() {
    let _home = HomeGuard::install();
    let (router, _state) = create_router();

    // `/dev/null/foo` can't host `repo.git` — `create_dir_all` in
    // `provision_local_workspace` fails with ENOTDIR, surfacing as 400 with
    // error_code=clone_failed. This exercises the real POST path without
    // getting as far as the daemon spawn.
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
    assert_eq!(body["path"], ws_path.to_string_lossy().as_ref());
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

// -- 17. POST failure does not leak placeholder state ----------------------

#[tokio::test]
#[serial(http_workspaces_home)]
async fn create_workspace_failure_cleans_up_placeholder() {
    let _home = HomeGuard::install();
    let (router, state) = create_router();

    // `/dev/null/...` can't host `repo.git` — `create_dir_all` in
    // `provision_local_workspace` fails with ENOTDIR, the handler takes the
    // rollback path and removes the placeholder. Without rollback, this would
    // leave a half-initialized `WorkspaceContext` in state visible to later
    // requests.
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
            system_prompt: None,
            introduction: None,
            env: Default::default(),
            error_message: None,
            session_usage: None,
            usage_summary: None,
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
