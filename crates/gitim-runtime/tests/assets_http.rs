#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::{Body, Bytes};
use axum::http::{header, Method, Request, StatusCode};
use futures::stream;
use gitim_core::types::{AssetRef, MAX_ASSET_REF_BYTES};
use gitim_runtime::assets::{AssetLimits, AssetService, AssetSource, AssetStore};
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::recover_from_config;
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::user_config::{UserConfig, WorkspaceEntry};
use gitim_runtime::workspace::WorkspaceContext;
use http_body_util::BodyExt;
use serde_json::Value;
use serial_test::serial;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
#[cfg(feature = "test-support")]
use std::sync::Barrier;
use tempfile::TempDir;
use tower::ServiceExt;

mod common;
use common::HomeGuard;

const RUNTIME_ID: &str = "24a6489c-762e-4461-9247-a824807a6080";
const CREATED_AT: &str = "2026-07-11T00:00:00Z";
const BOUNDARY: &str = "gitim-assets-boundary";
const PNG_1X1: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, b'I', b'D', b'A', b'T', 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, b'I', b'E',
    b'N', b'D', 0xae, 0x42, 0x60, 0x82,
];

struct Fixture {
    app: axum::Router,
    state: SharedRuntimeState,
    workspace: TempDir,
}

fn limits() -> AssetLimits {
    AssetLimits {
        workspace_quota_bytes: 256 * 1024 * 1024,
        min_free_bytes: 1,
        ..AssetLimits::default()
    }
}

fn fixture_with_limits(limits: AssetLimits) -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let (app, state) = create_router();
    let mut ctx = WorkspaceContext::new(
        "room".to_string(),
        "Room".to_string(),
        workspace.path().to_path_buf(),
    );
    ctx.git_config = Some(WorkspaceConfig {
        workspace: workspace.path().to_string_lossy().into_owned(),
        created_at: CREATED_AT.to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    });
    {
        let mut runtime = state.lock().unwrap();
        runtime.runtime_id = RUNTIME_ID.to_string();
        runtime.assets = Arc::new(AssetService::new(limits));
        runtime.workspaces.insert("room".to_string(), ctx);
    }
    Fixture {
        app,
        state,
        workspace,
    }
}

fn fixture() -> Fixture {
    fixture_with_limits(limits())
}

fn multipart_body(fields: &[(&str, &str, &str, &[u8])]) -> Vec<u8> {
    let mut body = Vec::new();
    for (field, filename, content_type, bytes) in fields {
        body.extend_from_slice(format!("--{BOUNDARY}\r\n").as_bytes());
        body.extend_from_slice(
            format!(
                "Content-Disposition: form-data; name=\"{field}\"; filename=\"{filename}\"\r\n"
            )
            .as_bytes(),
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{BOUNDARY}--\r\n").as_bytes());
    body
}

fn upload_request_with_body(body: Body) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/workspaces/room/assets")
        .header(
            header::CONTENT_TYPE,
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(body)
        .unwrap()
}

fn upload_request(fields: &[(&str, &str, &str, &[u8])]) -> Request<Body> {
    upload_request_with_body(Body::from(multipart_body(fields)))
}

fn allowed_upload(fields: &[(&str, &str, &str, &[u8])]) -> Request<Body> {
    let mut request = upload_request(fields);
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://gitim.io".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-mode", "cors".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-dest", "empty".parse().unwrap());
    request
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

async fn response_bytes(response: axum::response::Response) -> Bytes {
    response.into_body().collect().await.unwrap().to_bytes()
}

async fn upload_one(app: &axum::Router, bytes: &[u8], name: &str) -> Value {
    let response = app
        .clone()
        .oneshot(allowed_upload(&[("file", name, "text/plain", bytes)]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response_json(response).await["assets"][0].clone()
}

fn resolve_uri(asset: &Value, query: &str) -> String {
    let origin = asset["ref"]
        .as_str()
        .unwrap()
        .parse::<AssetRef>()
        .unwrap()
        .origin_runtime_id;
    format!(
        "/workspaces/room/assets/resolve/{origin}/{}{}",
        asset["sha256"].as_str().unwrap(),
        query
    )
}

fn object_files(workspace: &Path) -> Vec<PathBuf> {
    let root = workspace.join(".gitim-runtime/assets/v1/objects/sha256");
    let mut files = Vec::new();
    let Ok(shards) = std::fs::read_dir(root) else {
        return files;
    };
    for shard in shards.flatten() {
        if let Ok(entries) = std::fs::read_dir(shard.path()) {
            files.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    files
}

#[tokio::test]
async fn malicious_origin_is_rejected_before_upload_body_is_consumed() {
    let fixture = fixture();
    let polled = Arc::new(AtomicBool::new(false));
    let body_polled = Arc::clone(&polled);
    let bytes = multipart_body(&[("file", "pixel.png", "image/png", PNG_1X1)]);
    let stream = stream::once(async move {
        body_polled.store(true, Ordering::Release);
        Ok::<Bytes, std::io::Error>(Bytes::from(bytes))
    });
    let mut request = upload_request_with_body(Body::from_stream(stream));
    request
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    request
        .headers_mut()
        .insert("sec-fetch-site", "cross-site".parse().unwrap());

    let response = fixture.app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!polled.load(Ordering::Acquire));
    assert!(object_files(fixture.workspace.path()).is_empty());
}

#[tokio::test]
async fn node_local_object_rejects_every_browser_context() {
    let fixture = fixture();
    for (header_name, header_value) in [
        ("origin", "https://gitim.io"),
        ("sec-fetch-site", "same-origin"),
        ("sec-fetch-mode", "cors"),
        ("sec-fetch-dest", "empty"),
        ("sec-fetch-user", "?1"),
    ] {
        let request = Request::builder()
            .uri(format!(
                "/workspaces/room/assets/objects/{}",
                "a".repeat(64)
            ))
            .header(header_name, header_value)
            .body(Body::empty())
            .unwrap();
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{header_name}");
    }
}

#[tokio::test]
async fn browser_origin_allowlist_is_exact() {
    let fixture = fixture();
    for origin in [
        "https://gitim.io",
        "https://www.gitim.io",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://[::1]:5173",
        "http://localhost:4173",
    ] {
        let mut request = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        request
            .headers_mut()
            .insert("sec-fetch-site", "cross-site".parse().unwrap());
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_ne!(response.status(), StatusCode::FORBIDDEN, "{origin}");
    }

    for origin in [
        "null",
        "https://gitim.io.evil.example",
        "https://evil.example/gitim.io",
        "https://user@gitim.io",
        "https://gitim.io/",
        "http://gitim.io",
        "https://localhost:5173",
        "http://localhost",
        "http://localhost:5174",
    ] {
        let mut request = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        request
            .headers_mut()
            .insert("sec-fetch-site", "cross-site".parse().unwrap());
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::FORBIDDEN, "{origin}");
    }

    let mut duplicate = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
    duplicate
        .headers_mut()
        .append(header::ORIGIN, "https://gitim.io".parse().unwrap());
    duplicate
        .headers_mut()
        .append(header::ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(
        fixture.app.oneshot(duplicate).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn allowed_origin_fetch_metadata_is_singleton_and_consistent() {
    let fixture = fixture();
    for (site, mode, dest, expected) in [
        (None, Some("cors"), Some("empty"), StatusCode::FORBIDDEN),
        (
            Some("none"),
            Some("cors"),
            Some("empty"),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("bogus"),
            Some("cors"),
            Some("empty"),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("cross-site,same-site"),
            Some("cors"),
            Some("empty"),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("cross-site"),
            Some("navigate"),
            Some("empty"),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("cross-site"),
            Some("cors"),
            Some("image"),
            StatusCode::FORBIDDEN,
        ),
        (
            Some("cross-site"),
            Some("cors"),
            Some("empty"),
            StatusCode::OK,
        ),
    ] {
        let mut request = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
        request
            .headers_mut()
            .insert(header::ORIGIN, "https://gitim.io".parse().unwrap());
        if let Some(site) = site {
            request
                .headers_mut()
                .insert("sec-fetch-site", site.parse().unwrap());
        }
        if let Some(mode) = mode {
            request
                .headers_mut()
                .insert("sec-fetch-mode", mode.parse().unwrap());
        }
        if let Some(dest) = dest {
            request
                .headers_mut()
                .insert("sec-fetch-dest", dest.parse().unwrap());
        }
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            expected,
            "site={site:?} mode={mode:?} dest={dest:?}"
        );
    }

    let mut duplicate = allowed_upload(&[("file", "a.txt", "text/plain", b"a")]);
    duplicate
        .headers_mut()
        .append("sec-fetch-site", "same-site".parse().unwrap());
    assert_eq!(
        fixture.app.oneshot(duplicate).await.unwrap().status(),
        StatusCode::FORBIDDEN
    );
}

struct EnvGuard(Option<std::ffi::OsString>);

impl EnvGuard {
    fn set(name: &str, value: &str) -> Self {
        let previous = std::env::var_os(name);
        std::env::set_var(name, value);
        Self(previous)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(value) => std::env::set_var("GITIM_WEB_ORIGINS", value),
            None => std::env::remove_var("GITIM_WEB_ORIGINS"),
        }
    }
}

#[tokio::test]
#[serial(gitim_web_origins)]
async fn configured_origins_are_additive_exact_and_never_wildcards() {
    let _guard = EnvGuard::set(
        "GITIM_WEB_ORIGINS",
        "https://chat.example, *, https://self.example:8443",
    );
    let fixture = fixture();
    for (origin, expected) in [
        ("https://chat.example", StatusCode::OK),
        ("https://self.example:8443", StatusCode::OK),
        ("https://self.example", StatusCode::FORBIDDEN),
        ("https://anything.example", StatusCode::FORBIDDEN),
    ] {
        let mut request = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
        request
            .headers_mut()
            .insert(header::ORIGIN, origin.parse().unwrap());
        request
            .headers_mut()
            .insert("sec-fetch-site", "cross-site".parse().unwrap());
        let response = fixture.app.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), expected, "{origin}");
    }
}

#[tokio::test]
async fn fetch_metadata_without_origin_is_rejected_but_originless_cli_is_allowed() {
    let fixture = fixture();
    let cli = fixture
        .app
        .clone()
        .oneshot(upload_request(&[("file", "cli.txt", "text/plain", b"cli")]))
        .await
        .unwrap();
    assert_eq!(cli.status(), StatusCode::OK);

    let mut browser = upload_request(&[("file", "web.txt", "text/plain", b"web")]);
    browser
        .headers_mut()
        .insert("sec-fetch-site", "same-origin".parse().unwrap());
    let rejected = fixture.app.oneshot(browser).await.unwrap();
    assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn only_resolve_get_accepts_exact_user_navigation_tuple() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, b"navigation", "note.txt").await;
    let uri = resolve_uri(&asset, "?name=note.txt&download=1");
    let navigation = |method: Method| {
        Request::builder()
            .method(method)
            .uri(&uri)
            .header("sec-fetch-site", "none")
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-user", "?1")
            .body(Body::empty())
            .unwrap()
    };
    assert_eq!(
        fixture
            .app
            .clone()
            .oneshot(navigation(Method::GET))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .app
            .clone()
            .oneshot(navigation(Method::HEAD))
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );

    for site in ["cross-site", "same-site", "same-origin"] {
        let request = Request::builder()
            .method(Method::GET)
            .uri(&uri)
            .header("sec-fetch-site", site)
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-user", "?1")
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::OK,
            "site={site}"
        );
    }

    for duplicate_header in [
        "sec-fetch-site",
        "sec-fetch-mode",
        "sec-fetch-dest",
        "sec-fetch-user",
    ] {
        let mut request = navigation(Method::GET);
        let duplicate_value = request.headers()[duplicate_header].clone();
        request
            .headers_mut()
            .append(duplicate_header, duplicate_value);
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "duplicate {duplicate_header}"
        );
    }

    // Task 6's authoritative plan grants the navigation exception to GET only.
    // HEAD remains available to allowed-origin browser fetches and originless peers.
    for site in [None, Some("bogus"), Some("cross-site,same-origin")] {
        let mut request = Request::builder()
            .method(Method::GET)
            .uri(&uri)
            .header("sec-fetch-mode", "navigate")
            .header("sec-fetch-dest", "document")
            .header("sec-fetch-user", "?1")
            .body(Body::empty())
            .unwrap();
        if let Some(site) = site {
            request
                .headers_mut()
                .insert("sec-fetch-site", site.parse().unwrap());
        }
        assert_eq!(
            fixture.app.clone().oneshot(request).await.unwrap().status(),
            StatusCode::FORBIDDEN,
            "site={site:?}"
        );
    }

    let mut upload_navigation = upload_request(&[("file", "a.txt", "text/plain", b"a")]);
    for (name, value) in [
        ("sec-fetch-site", "none"),
        ("sec-fetch-mode", "navigate"),
        ("sec-fetch-dest", "document"),
        ("sec-fetch-user", "?1"),
    ] {
        upload_navigation
            .headers_mut()
            .insert(name, value.parse().unwrap());
    }
    assert_eq!(
        fixture
            .app
            .oneshot(upload_navigation)
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn upload_returns_canonical_refs_in_file_field_order() {
    let fixture = fixture();
    let response = fixture
        .app
        .oneshot(allowed_upload(&[
            ("file", "first.txt", "image/png", b"first"),
            ("file", "pixel.png", "text/plain", PNG_1X1),
        ]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["ok"], true);
    let assets = body["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 2);
    assert_eq!(assets[0]["name"], "first.txt");
    assert_eq!(assets[0]["media_type"], "application/octet-stream");
    assert_eq!(assets[1]["name"], "pixel.png");
    assert_eq!(assets[1]["media_type"], "image/png");
    assert_eq!(assets[1]["width"], 1);
    assert_eq!(assets[1]["height"], 1);
    for asset in assets {
        let raw = asset["ref"].as_str().unwrap();
        assert!(raw.parse::<AssetRef>().is_ok());
        assert!(raw.len() <= MAX_ASSET_REF_BYTES);
    }
}

#[tokio::test]
async fn upload_without_a_usable_filename_falls_back_to_attachment() {
    let fixture = fixture();
    let body = format!(
        "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"file\"\r\n\r\nbytes\r\n--{BOUNDARY}--\r\n"
    );
    let response = fixture
        .app
        .oneshot(upload_request_with_body(Body::from(body)))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["assets"][0]["name"],
        "attachment"
    );
}

#[tokio::test]
async fn upload_rejects_unknown_empty_and_more_than_ten_fields_without_persisting() {
    for fields in [
        Vec::new(),
        vec![("caption", "a.txt", "text/plain", b"a".as_slice())],
        vec![
            ("file", "a.txt", "text/plain", b"a".as_slice()),
            ("caption", "b.txt", "text/plain", b"b".as_slice()),
        ],
        (0..11)
            .map(|_| ("file", "a.txt", "text/plain", b"a".as_slice()))
            .collect::<Vec<_>>(),
    ] {
        let fixture = fixture();
        let response = fixture.app.oneshot(allowed_upload(&fields)).await.unwrap();
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::UNPROCESSABLE_ENTITY
            ),
            "status was {}",
            response.status()
        );
        assert!(object_files(fixture.workspace.path()).is_empty());
    }
}

#[tokio::test]
async fn upload_accepts_exactly_ten_files_and_enforces_encoded_filename_boundaries() {
    let fixture = fixture();
    let ten = (0..10)
        .map(|_| ("file", "a.txt", "text/plain", b"a".as_slice()))
        .collect::<Vec<_>>();
    let response = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&ten))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await["assets"]
            .as_array()
            .unwrap()
            .len(),
        10
    );

    let encoded_max = "€".repeat(85);
    assert_eq!(encoded_max.len(), 255);
    let response = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&[(
            "file",
            encoded_max.as_str(),
            "text/plain",
            b"x",
        )]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let asset = response_json(response).await["assets"][0].clone();
    let canonical = asset["ref"].as_str().unwrap();
    assert!(canonical.len() <= MAX_ASSET_REF_BYTES);
    assert!(canonical.parse::<AssetRef>().is_ok());

    let too_long = "é".repeat(128);
    assert_eq!(too_long.len(), 256);
    let response = fixture
        .app
        .oneshot(allowed_upload(&[(
            "file",
            too_long.as_str(),
            "text/plain",
            b"y",
        )]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(response_json(response).await["error_code"], "invalid_asset");
}

#[tokio::test]
async fn upload_enforces_file_and_aggregate_streaming_limits_atomically() {
    let constrained = AssetLimits {
        max_file_bytes: 4,
        max_request_bytes: 6,
        ..limits()
    };
    let fixture = fixture_with_limits(constrained);

    let exact = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&[(
            "file",
            "four.bin",
            "text/plain",
            b"1234",
        )]))
        .await
        .unwrap();
    assert_eq!(exact.status(), StatusCode::OK);

    let over_file = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&[(
            "file",
            "five.bin",
            "text/plain",
            b"12345",
        )]))
        .await
        .unwrap();
    assert_eq!(over_file.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(over_file).await["error_code"],
        "asset_too_large"
    );

    let over_request = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&[
            ("file", "one.bin", "text/plain", b"1234"),
            ("file", "two.bin", "text/plain", b"567"),
        ]))
        .await
        .unwrap();
    assert_eq!(over_request.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(over_request).await["error_code"],
        "asset_request_too_large"
    );
    assert_eq!(object_files(fixture.workspace.path()).len(), 1);

    let quota_fixture = fixture_with_limits(AssetLimits {
        workspace_quota_bytes: 3,
        max_file_bytes: 4,
        max_request_bytes: 4,
        ..limits()
    });
    let quota = quota_fixture
        .app
        .oneshot(allowed_upload(&[(
            "file",
            "four.bin",
            "text/plain",
            b"1234",
        )]))
        .await
        .unwrap();
    assert_eq!(quota.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(
        response_json(quota).await["error_code"],
        "asset_quota_exceeded"
    );
    assert!(object_files(quota_fixture.workspace.path()).is_empty());
}

#[tokio::test]
async fn asset_body_override_does_not_change_the_non_asset_body_limit() {
    let fixture = fixture();
    let payload = vec![b'x'; 2 * 1024 * 1024 + 16];
    let upload = fixture
        .app
        .clone()
        .oneshot(upload_request(&[(
            "file",
            "large.bin",
            "application/octet-stream",
            payload.as_slice(),
        )]))
        .await
        .unwrap();
    assert_eq!(upload.status(), StatusCode::OK);

    let oversized_json = serde_json::to_vec(&serde_json::json!({
        "path": "/definitely/not/a/workspace",
        "git": { "provider": "local" },
        "padding": "x".repeat(2 * 1024 * 1024),
    }))
    .unwrap();
    let non_asset = fixture
        .app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/workspaces")
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(oversized_json))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(non_asset.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn local_get_head_range_and_exact_strong_etag_use_verified_metadata() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, PNG_1X1, "pixel.png").await;
    let uri = resolve_uri(&asset, "?name=pixel.png");
    let etag = format!("\"sha256-{}\"", asset["sha256"].as_str().unwrap());

    let get = fixture
        .app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(get.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(get.headers()["x-content-type-options"], "nosniff");
    assert_eq!(
        get.headers()[header::CACHE_CONTROL],
        "private, immutable, max-age=31536000"
    );
    assert_eq!(get.headers()[header::ETAG], etag);
    assert!(get.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("inline;"));
    assert_eq!(response_bytes(get).await.as_ref(), PNG_1X1);

    let head = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        PNG_1X1.len().to_string()
    );
    assert!(response_bytes(head).await.is_empty());

    for (range, expected_status, expected_len) in [
        ("bytes=0-7", StatusCode::PARTIAL_CONTENT, Some(8)),
        ("bytes=-4", StatusCode::PARTIAL_CONTENT, Some(4)),
        ("garbage", StatusCode::RANGE_NOT_SATISFIABLE, Some(0)),
        ("bytes=0-1,3-4", StatusCode::RANGE_NOT_SATISFIABLE, None),
    ] {
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(&uri)
                    .header(header::RANGE, range)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected_status, "{range}");
        let body = response_bytes(response).await;
        if let Some(expected_len) = expected_len {
            assert_eq!(body.len(), expected_len, "{range}");
        }
    }

    let not_modified = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert!(response_bytes(not_modified).await.is_empty());

    let weak = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(&uri)
                .header(header::IF_NONE_MATCH, format!("W/{etag}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(weak.status(), StatusCode::OK);

    let mut duplicate_etag = Request::builder().uri(&uri).body(Body::empty()).unwrap();
    duplicate_etag
        .headers_mut()
        .append(header::IF_NONE_MATCH, etag.parse().unwrap());
    duplicate_etag
        .headers_mut()
        .append(header::IF_NONE_MATCH, "\"another\"".parse().unwrap());
    assert_eq!(
        fixture.app.oneshot(duplicate_etag).await.unwrap().status(),
        StatusCode::OK
    );
}

#[tokio::test]
async fn local_hash_wins_even_when_the_origin_hint_names_another_runtime() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, PNG_1X1, "pixel.png").await;
    let uri = format!(
        "/workspaces/room/assets/resolve/{}/{}",
        "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
        asset["sha256"].as_str().unwrap()
    );
    let response = fixture
        .app
        .clone()
        .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await.as_ref(), PNG_1X1);

    let missing = fixture
        .app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/workspaces/room/assets/resolve/{}/{}",
                    "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
                    "a".repeat(64)
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(missing).await["error_code"], "asset_missing");
}

#[tokio::test]
async fn unsafe_types_force_attachment_and_unicode_filename_is_rfc5987_encoded() {
    let fixture = fixture();
    for (bytes, name, mime) in [
        (
            b"<svg xmlns='http://www.w3.org/2000/svg'></svg>".as_slice(),
            "图.svg",
            "image/svg+xml",
        ),
        (
            b"<!doctype html><title>x</title>".as_slice(),
            "图.html",
            "text/html",
        ),
        (b"unknown".as_slice(), "图.bin", "application/octet-stream"),
    ] {
        let asset = upload_one(&fixture.app, bytes, name).await;
        assert_eq!(asset["media_type"], mime);
        let response = fixture
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(resolve_uri(&asset, "?name=%E5%9B%BE.bin"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let disposition = response.headers()[header::CONTENT_DISPOSITION]
            .to_str()
            .unwrap();
        assert!(disposition.starts_with("attachment;"), "{disposition}");
        assert!(
            disposition.contains("filename*=UTF-8''%E5%9B%BE.bin"),
            "{disposition}"
        );
    }

    let image = upload_one(&fixture.app, PNG_1X1, "pixel.png").await;
    let forced = fixture
        .app
        .oneshot(
            Request::builder()
                .uri(resolve_uri(&image, "?name=pixel.png&download=1"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(forced.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .starts_with("attachment;"));
}

#[tokio::test]
async fn invalid_route_parameters_and_unknown_workspace_are_typed() {
    let fx = fixture();
    for uri in [
        format!(
            "/workspaces/room/assets/resolve/not-a-uuid/{}",
            "a".repeat(64)
        ),
        format!("/workspaces/room/assets/resolve/{RUNTIME_ID}/ABC"),
        format!(
            "/workspaces/room/assets/resolve/{RUNTIME_ID}/{}?name=../secret",
            "a".repeat(64)
        ),
        format!(
            "/workspaces/room/assets/resolve/{RUNTIME_ID}/{}?name=evil%0D%0AX-Test%3Ayes",
            "a".repeat(64)
        ),
        format!(
            "/workspaces/room/assets/resolve/{RUNTIME_ID}/{}?name={}",
            "a".repeat(64),
            "x".repeat(MAX_ASSET_REF_BYTES + 1)
        ),
    ] {
        let response = fx
            .app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response_json(response).await["error_code"],
            "invalid_asset_ref"
        );
    }

    let response = fx
        .app
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/workspaces/missing/assets")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={BOUNDARY}"),
                )
                .body(Body::from(multipart_body(&[(
                    "file",
                    "a.txt",
                    "text/plain",
                    b"a",
                )])))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(response).await["error_code"],
        "workspace_not_found"
    );

    let binding_fixture = fixture();
    binding_fixture
        .state
        .lock()
        .unwrap()
        .workspaces
        .get_mut("room")
        .unwrap()
        .git_config = None;
    let response = binding_fixture
        .app
        .oneshot(upload_request(&[("file", "a.txt", "text/plain", b"a")]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        response_json(response).await["error_code"],
        "workspace_not_initialized"
    );
}

#[tokio::test]
async fn node_local_object_is_originless_local_only_and_has_same_headers() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, PNG_1X1, "pixel.png").await;
    fixture.state.lock().unwrap().runtime_id.clear();
    let uri = format!(
        "/workspaces/room/assets/objects/{}",
        asset["sha256"].as_str().unwrap()
    );
    let response = fixture
        .app
        .clone()
        .oneshot(Request::builder().uri(&uri).body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");

    let missing = fixture
        .app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/workspaces/room/assets/objects/{}",
                    "a".repeat(64)
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    assert_eq!(response_json(missing).await["error_code"], "asset_missing");
}

#[tokio::test]
async fn content_disposition_percent_encodes_non_attr_characters() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, PNG_1X1, "pixel.png").await;
    let response = fixture
        .app
        .oneshot(
            Request::builder()
                .uri(resolve_uri(&asset, "?name=a%3Ab%3Fc%3Dd.txt"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.headers()[header::CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .contains("filename*=UTF-8''a%3Ab%3Fc%3Dd.txt"));
}

#[tokio::test]
async fn health_reports_cached_asset_usage_and_real_counters_without_rescanning() {
    let fixture = fixture();
    let asset = upload_one(&fixture.app, b"health", "health.bin").await;
    let initial = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(initial.status(), StatusCode::OK);
    let body = response_json(initial).await;
    assert_eq!(body["asset_store_failures"], 0);
    assert_eq!(body["asset_hash_mismatches"], 0);
    assert_eq!(body["asset_fleet_fetch_failures"], 0);
    assert_eq!(body["workspace_epochs"][0]["asset_bytes"], asset["size"]);
    assert_eq!(body["workspace_epochs"][0]["asset_objects"], 1);
    assert_eq!(
        body["workspace_epochs"][0]["asset_quota"],
        256 * 1024 * 1024
    );

    let relocated = fixture.workspace.path().with_extension("health-relocated");
    std::fs::rename(fixture.workspace.path(), &relocated).unwrap();
    let health_without_workspace_path = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        response_json(health_without_workspace_path).await["workspace_epochs"][0]["asset_bytes"],
        asset["size"]
    );
    std::fs::rename(&relocated, fixture.workspace.path()).unwrap();

    let object = object_files(fixture.workspace.path()).pop().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(10));
    std::fs::write(&object, b"broken").unwrap();
    let failed = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri(resolve_uri(&asset, ""))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(failed.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let health = fixture
        .app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_json(health).await["asset_store_failures"], 1);
}

#[tokio::test]
async fn local_persistence_failure_is_507_sanitized_and_returns_no_refs() {
    let fixture = fixture();
    upload_one(&fixture.app, b"seed", "seed.bin").await;
    let bytes = b"persistence-failure";
    let hash = format!("{:x}", Sha256::digest(bytes));
    let shard = fixture
        .workspace
        .path()
        .join(".gitim-runtime/assets/v1/metadata/sha256")
        .join(&hash[..2]);
    assert!(
        !shard.exists(),
        "fixture hash unexpectedly reused seed shard"
    );
    std::fs::write(&shard, b"not-a-directory").unwrap();

    let response = fixture
        .app
        .clone()
        .oneshot(allowed_upload(&[(
            "file",
            "failure.bin",
            "application/octet-stream",
            bytes,
        )]))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::INSUFFICIENT_STORAGE);
    let body = response_json(response).await;
    assert_eq!(body["error_code"], "asset_store_failed");
    assert!(body.get("assets").is_none());
    assert!(!body["error"]
        .as_str()
        .unwrap()
        .contains(fixture.workspace.path().to_string_lossy().as_ref()));

    let health = fixture
        .app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response_json(health).await["asset_store_failures"], 1);
}

#[tokio::test]
#[serial(asset_recovery_home)]
async fn startup_recovery_opens_and_caches_the_bound_asset_store() {
    let home = HomeGuard::install();
    let workspace = tempfile::tempdir().unwrap();
    let config = WorkspaceConfig {
        workspace: workspace.path().to_string_lossy().into_owned(),
        created_at: CREATED_AT.to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    };
    config.write(workspace.path()).unwrap();
    let recovered_size = b"recovered".len() as u64;
    AssetStore::open(
        workspace.path(),
        format!("local:{CREATED_AT}"),
        AssetLimits::default(),
    )
    .unwrap()
    .put_bytes(b"recovered", AssetSource::LocalUpload)
    .unwrap();
    gitim_runtime::user_config::write(&UserConfig {
        runtime_id: RUNTIME_ID.to_string(),
        workspaces: vec![WorkspaceEntry {
            slug: "room".to_string(),
            workspace_name: "Room".to_string(),
            path: workspace.path().to_string_lossy().into_owned(),
        }],
        listen_port: None,
        fleet_nodes: Vec::new(),
    })
    .unwrap();
    assert!(home.path().join(".gitim/runtime.json").is_file());
    let (app, state) = create_router();
    state.lock().unwrap().runtime_id = RUNTIME_ID.to_string();

    recover_from_config(Arc::clone(&state)).await;

    let service = Arc::clone(&state.lock().unwrap().assets);
    assert_eq!(
        service.cached_usage(workspace.path()),
        Some(gitim_runtime::assets::AssetUsage {
            bytes: recovered_size,
            objects: 1,
        })
    );
    assert!(workspace
        .path()
        .join(".gitim-runtime/assets/v1/store.json")
        .is_file());
    let health = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let health = response_json(health).await;
    assert_eq!(health["workspace_epochs"][0]["asset_bytes"], recovered_size);
    assert_eq!(health["workspace_epochs"][0]["asset_objects"], 1);
}

#[cfg(feature = "test-support")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn health_does_not_wait_for_the_store_operation_gate() {
    let fixture = fixture();
    let service = Arc::clone(&fixture.state.lock().unwrap().assets);
    let store = service
        .open_store(fixture.workspace.path(), format!("local:{CREATED_AT}"))
        .unwrap();
    let staged = store.stage_bytes("gate.bin", b"gate").await.unwrap();
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let persist = tokio::spawn({
        let store = store.clone();
        async move { store.persist_staged(staged, AssetSource::LocalUpload).await }
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();

    let health = tokio::spawn(
        fixture.app.clone().oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        ),
    );
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let completed_without_store_gate = health.is_finished();
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    persist.await.unwrap().unwrap();
    assert_eq!(health.await.unwrap().unwrap().status(), StatusCode::OK);
    assert!(completed_without_store_gate);
}
