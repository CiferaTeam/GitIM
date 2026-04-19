//! Integration tests for `POST /runtime/update-and-restart` (Task 6 sync phase).
//!
//! ## What's covered
//!
//! - The **install-dir strict check**: several flavours of non-canonical,
//!   dev-tree, and missing paths all get `403 runtime_not_installed`.
//! - The **concurrency guard primitive**: the `AtomicBool::swap` contract the
//!   handler relies on for 409. (We don't exercise the full HTTP 409 path
//!   here — see `atomic_swap_supports_guard_contract` and the inline note in
//!   `update.rs` for why.)
//! - The **status-code mapping**: every error code declared in the plan has
//!   a matching HTTP status documented via a unit assertion in `update.rs`
//!   (`status_for_maps_expected_codes` — lib test).
//!
//! ## What's intentionally not covered here
//!
//! The unsupported-platform / network / already-latest / download / extract
//! / archive-missing / sanity branches all live **past** the install-dir
//! check. Reaching them from an integration test requires either (a)
//! mocking `dirs::home_dir()` — which is a process-global `$HOME` swap and
//! fights with every other HOME-sensitive test in this crate — or (b)
//! mocking GitHub's releases API at the network layer (wiremock).
//!
//! Neither pays off at the unit/integration boundary here because those
//! branches are already covered one level down:
//!
//! - `gitim-updater` has wiremock coverage of `fetch_latest_tag`,
//!   `download_and_extract`, and `HttpStatus` vs `Network` error shapes.
//! - `update::tests` (lib) covers `strict_install_dir_check`,
//!   `status_for`, and `sanity_check_new_runtime` with fake
//!   `sleep`/`echo` processes.
//!
//! So the integration-layer tests focus on the *handler surface*: the
//! strict-mode gate, response shape, and concurrency flag — the things
//! that would change silently if the handler were re-wired without care.

use std::path::PathBuf;
use std::sync::atomic::Ordering;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tempfile::TempDir;
use tower::ServiceExt;

use gitim_runtime::http::create_router_with_exe;

/// POST to the endpoint via `oneshot` and return `(status, json_body)`.
async fn post_update(router: axum::Router) -> (StatusCode, Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/runtime/update-and-restart")
        .body(Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

/// Build a router whose `canonical_exe_path` points inside `tmp` — i.e. a
/// definitely-not-`~/.gitim/bin` location. Used to exercise the strict-mode
/// reject branch without touching `$HOME`.
fn router_with_tempdir_exe(tmp: &TempDir) -> axum::Router {
    let exe = tmp.path().join("gitim-runtime");
    std::fs::write(&exe, b"fake binary for test").unwrap();
    let canonical = exe.canonicalize().unwrap();
    let (router, _state) = create_router_with_exe(canonical);
    router
}

// -- install-dir strict check rejects outside-home paths -------------------

#[tokio::test]
async fn rejects_runtime_in_tempdir() {
    let tmp = TempDir::new().unwrap();
    let router = router_with_tempdir_exe(&tmp);
    let (status, body) = post_update(router).await;

    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error_code"], "runtime_not_installed");
    assert!(
        body["detail"].as_str().is_some(),
        "detail should be a human-readable string: {body:#?}"
    );
}

#[tokio::test]
async fn rejects_runtime_whose_exe_has_no_parent() {
    // A path with no parent — extremely pathological but the strict check
    // has an explicit branch for it that we want to keep exercised.
    let (router, _state) = create_router_with_exe(PathBuf::from("/"));
    let (status, body) = post_update(router).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error_code"], "runtime_not_installed");
}

#[tokio::test]
async fn rejects_runtime_with_nonexistent_parent() {
    // Parent doesn't exist → canonicalize fails → 403 with
    // runtime_not_installed. Exercises the canonicalize-failure branch of
    // strict_install_dir_check distinct from the "path exists but isn't
    // ~/.gitim/bin" branch.
    let (router, _state) = create_router_with_exe(PathBuf::from(
        "/definitely/nonexistent/path-for-task6/gitim-runtime",
    ));
    let (status, body) = post_update(router).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(body["error_code"], "runtime_not_installed");
}

// -- concurrency guard ------------------------------------------------------

#[tokio::test]
async fn atomic_swap_supports_guard_contract() {
    // Flip the guard before calling. A real update-in-flight would have
    // already set this; we simulate that state without needing to race two
    // requests (which would be flaky on CI). The install-dir check runs
    // BEFORE the concurrency check, so we set up a path that would pass the
    // strict check... except we can't do that without `$HOME` mocking.
    //
    // Instead: verify the ordering. With a bogus install dir, we get 403
    // (install-dir rejection happens first). With a valid install dir AND
    // in_progress=true we'd get 409. Since we can't easily get a valid
    // install dir here, we assert the more important invariant: the guard
    // itself is reachable from the router's shared state and its behaviour
    // (swap semantics) is correct.
    let tmp = TempDir::new().unwrap();
    let exe = tmp.path().join("gitim-runtime");
    std::fs::write(&exe, b"fake").unwrap();
    let canonical = exe.canonicalize().unwrap();
    let (_router, state) = create_router_with_exe(canonical);

    // Confirm the flag starts cleared (default construction behaviour).
    let guard = state.lock().unwrap().update_in_progress.clone();
    assert!(!guard.load(Ordering::SeqCst));

    // After one swap the flag is live; a second swap returns `true` (meaning
    // a concurrent caller would see "already in progress") — this is exactly
    // what the handler relies on for 409.
    let prev = guard.swap(true, Ordering::SeqCst);
    assert!(!prev, "initial state must be false");
    let prev = guard.swap(true, Ordering::SeqCst);
    assert!(prev, "second swap must observe true");
}

// -- response body shape ----------------------------------------------------

#[tokio::test]
async fn error_response_body_has_error_code_and_detail_only() {
    // The plan calls the response schema `{ error_code, detail }`. Lock in
    // the field set: no accidental `"ok": false` leak, no stray nested
    // objects. Cheap regression guard.
    let tmp = TempDir::new().unwrap();
    let router = router_with_tempdir_exe(&tmp);
    let (_status, body) = post_update(router).await;
    let obj = body.as_object().expect("error body must be a JSON object");
    let keys: std::collections::HashSet<&str> = obj.keys().map(String::as_str).collect();
    assert_eq!(
        keys,
        ["error_code", "detail"].into_iter().collect(),
        "unexpected error body shape: {body:#?}"
    );
}
