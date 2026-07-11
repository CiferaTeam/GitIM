#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use gitim_core::types::{
    AssetRef, ASSET_REF_VERSION, MAX_ASSETS_PER_MESSAGE, MAX_ASSET_REQUEST_BYTES,
};
use gitim_runtime::assets::{AssetLimits, AssetService};
use gitim_runtime::cli::UploadFile;
use gitim_runtime::cli::{cmd_asset, CliError, Client};
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::workspace::WorkspaceContext;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RUNTIME_ID: &str = "24a6489c-762e-4461-9247-a824807a6080";
const CREATED_AT: &str = "2026-07-12T00:00:00Z";

struct RuntimeFixture {
    addr: SocketAddr,
    state: SharedRuntimeState,
    workspace: TempDir,
    server: tokio::task::JoinHandle<()>,
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn spawn_runtime() -> RuntimeFixture {
    let workspace = tempfile::tempdir().unwrap();
    let workspace_path = workspace.path().canonicalize().unwrap();
    let (router, state) = create_router();
    let mut context = WorkspaceContext::new(
        "room".to_string(),
        "Room".to_string(),
        workspace_path.clone(),
    );
    context.git_config = Some(WorkspaceConfig {
        workspace: workspace_path.to_string_lossy().into_owned(),
        created_at: CREATED_AT.to_string(),
        git: GitConfig {
            provider: GitProvider::Local,
            remote_url: None,
            token: None,
            github_email: None,
        },
    });
    let service = Arc::new(AssetService::new(AssetLimits {
        workspace_quota_bytes: 256 * 1024 * 1024,
        min_free_bytes: 1,
        ..AssetLimits::default()
    }));
    service
        .activate_workspace(
            &workspace_path,
            format!("local:{CREATED_AT}"),
            &context.asset_token,
        )
        .unwrap();
    {
        let mut runtime = state.lock().unwrap();
        runtime.runtime_id = RUNTIME_ID.to_string();
        runtime.assets = service;
        runtime.workspaces.insert("room".to_string(), context);
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    RuntimeFixture {
        addr,
        state,
        workspace,
        server,
    }
}

fn runtime_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_gitim-runtime"))
}

fn remove_proxy_environment(command: &mut Command) {
    for name in [
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
    ] {
        command.env_remove(name);
    }
}

async fn run_cli(addr: SocketAddr, cwd: PathBuf, args: Vec<String>) -> std::process::Output {
    tokio::task::spawn_blocking(move || {
        let mut command = Command::new(runtime_bin());
        remove_proxy_environment(&mut command);
        command
            .current_dir(cwd)
            .env("GITIM_RUNTIME_PORT", addr.port().to_string())
            .args(args)
            .output()
            .expect("spawn gitim-runtime")
    })
    .await
    .unwrap()
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fifo_is_rejected_without_blocking_or_contacting_runtime() {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let files = tempfile::tempdir().unwrap();
    let fifo = files.path().join("asset.fifo");
    let fifo_c = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(fifo_c.as_ptr(), 0o600) }, 0);
    let client = Client::new("http://127.0.0.1:1".to_string());
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        cmd_asset::put(
            &client,
            cmd_asset::PutArgs {
                workspace: Some("room".to_string()),
                files: vec![fifo.clone()],
            },
        ),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => {
            use std::os::unix::fs::OpenOptionsExt;
            let _release = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NONBLOCK)
                .open(&fifo)
                .unwrap();
            panic!("asset put blocked while opening a FIFO");
        }
    };
    let error = result.expect_err("FIFO must be rejected");
    assert!(matches!(error, CliError::InvalidConfig(_)));
    assert!(error.to_string().contains("regular file"));
}

fn write(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
}

fn stdout_json(output: &std::process::Output) -> Value {
    assert!(
        output.status.success(),
        "status={:?}, stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("stdout is canonical JSON")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_preserves_repeat_order_and_get_round_trips_with_default_workspace() {
    let fixture = spawn_runtime().await;
    let files = tempfile::tempdir().unwrap();
    let first = files.path().join("first.txt");
    let second = files.path().join("second.bin");
    write(&first, b"first bytes");
    write(&second, b"second bytes");

    let output = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "put".into(),
            "--file".into(),
            first.to_string_lossy().into_owned(),
            "--file".into(),
            second.to_string_lossy().into_owned(),
        ],
    )
    .await;
    let body = stdout_json(&output);
    assert_eq!(body["ok"], true);
    assert_eq!(body["assets"][0]["name"], "first.txt");
    assert_eq!(body["assets"][1]["name"], "second.bin");
    let first_ref = body["assets"][0]["ref"].as_str().unwrap().to_string();

    let download_dir = tempfile::tempdir().unwrap();
    let get = run_cli(
        fixture.addr,
        download_dir.path().to_path_buf(),
        vec!["asset".into(), "get".into(), "--ref".into(), first_ref],
    )
    .await;
    let get_body = stdout_json(&get);
    assert_eq!(get_body["ok"], true);
    assert_eq!(get_body["path"], "first.txt");
    assert_eq!(
        std::fs::read(download_dir.path().join("first.txt")).unwrap(),
        b"first bytes"
    );
    assert!(!fixture.workspace.path().join("human/first.txt").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn get_refuses_existing_destination_and_force_replaces_atomically() {
    let fixture = spawn_runtime().await;
    let files = tempfile::tempdir().unwrap();
    let source = files.path().join("source.txt");
    write(&source, b"new content");
    let put = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "put".into(),
            "--workspace".into(),
            "room".into(),
            "--file".into(),
            source.to_string_lossy().into_owned(),
        ],
    )
    .await;
    let asset_ref = stdout_json(&put)["assets"][0]["ref"]
        .as_str()
        .unwrap()
        .to_string();
    let destination = files.path().join("destination.txt");
    write(&destination, b"keep me");

    let refused = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "get".into(),
            "--ref".into(),
            asset_ref.clone(),
            "--output".into(),
            destination.to_string_lossy().into_owned(),
        ],
    )
    .await;
    assert_eq!(refused.status.code(), Some(1));
    assert_eq!(std::fs::read(&destination).unwrap(), b"keep me");

    let forced = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "get".into(),
            "--ref".into(),
            asset_ref,
            "--output".into(),
            destination.to_string_lossy().into_owned(),
            "--force".into(),
        ],
    )
    .await;
    stdout_json(&forced);
    assert_eq!(std::fs::read(&destination).unwrap(), b"new content");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_rejects_more_than_ten_files_before_upload() {
    let fixture = spawn_runtime().await;
    let files = tempfile::tempdir().unwrap();
    let mut args = vec!["asset".to_string(), "put".to_string()];
    for index in 0..=MAX_ASSETS_PER_MESSAGE {
        let path = files.path().join(format!("{index}.txt"));
        write(&path, b"x");
        args.push("--file".to_string());
        args.push(path.to_string_lossy().into_owned());
    }

    let output = run_cli(fixture.addr, files.path().to_path_buf(), args).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("10"));
    let object_root = fixture
        .workspace
        .path()
        .join(".gitim-runtime/assets/v1/objects/sha256");
    assert!(!object_root.exists() || std::fs::read_dir(object_root).unwrap().next().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn put_rejects_non_regular_and_aggregate_oversize_before_upload() {
    let fixture = spawn_runtime().await;
    let files = tempfile::tempdir().unwrap();

    let directory = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "put".into(),
            "--workspace".into(),
            "room".into(),
            "--file".into(),
            files.path().to_string_lossy().into_owned(),
        ],
    )
    .await;
    assert_eq!(directory.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&directory.stderr).contains("regular file"));

    let mut args = vec![
        "asset".to_string(),
        "put".to_string(),
        "--workspace".to_string(),
        "room".to_string(),
    ];
    let length = MAX_ASSET_REQUEST_BYTES / 5 + 1;
    for index in 0..5 {
        let path = files.path().join(format!("large-{index}.bin"));
        std::fs::File::create(&path)
            .unwrap()
            .set_len(length)
            .unwrap();
        args.push("--file".to_string());
        args.push(path.to_string_lossy().into_owned());
    }
    let aggregate = run_cli(fixture.addr, files.path().to_path_buf(), args).await;
    assert_eq!(aggregate.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&aggregate.stderr).contains("request limit"));

    let object_root = fixture
        .workspace
        .path()
        .join(".gitim-runtime/assets/v1/objects/sha256");
    assert!(!object_root.exists() || std::fs::read_dir(object_root).unwrap().next().is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn omitted_workspace_is_ambiguous_when_multiple_are_configured() {
    let fixture = spawn_runtime().await;
    fixture.state.lock().unwrap().workspaces.insert(
        "other".to_string(),
        WorkspaceContext::new(
            "other".to_string(),
            "Other".to_string(),
            fixture.workspace.path().join("other"),
        ),
    );
    let files = tempfile::tempdir().unwrap();
    let source = files.path().join("source.txt");
    write(&source, b"content");
    let output = run_cli(
        fixture.addr,
        files.path().to_path_buf(),
        vec![
            "asset".into(),
            "put".into(),
            "--file".into(),
            source.to_string_lossy().into_owned(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("multiple workspaces"));
    assert!(stderr.contains("--workspace"));
}

fn reference_for(bytes: &[u8], name: &str) -> AssetRef {
    AssetRef {
        version: ASSET_REF_VERSION,
        origin_runtime_id: RUNTIME_ID.to_string(),
        sha256: format!("{:x}", Sha256::digest(bytes)),
        name: name.to_string(),
        media_type: "application/octet-stream".to_string(),
        size: bytes.len() as u64,
        width: None,
        height: None,
    }
}

async fn spawn_download_server(
    asset_ref: &AssetRef,
    response: axum::response::Response,
) -> (Client, tokio::task::JoinHandle<()>) {
    let origin = asset_ref.origin_runtime_id.clone();
    let hash = asset_ref.sha256.clone();
    let route = format!("/workspaces/room/assets/resolve/{origin}/{hash}");
    let response = Arc::new(Mutex::new(Some(response)));
    let route_response = Arc::clone(&response);
    let app = Router::new()
        .route(
            "/workspaces",
            get(|| async { Json(json!({"workspaces": [{"slug": "room"}]})) }),
        )
        .route(
            &route,
            get(move || {
                let response = route_response
                    .lock()
                    .unwrap()
                    .take()
                    .expect("one download request");
                async move { response }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (Client::new(format!("http://{addr}")), handle)
}

async fn spawn_aborting_download_server(
    asset_ref: &AssetRef,
) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_path = format!(
        "/workspaces/room/assets/resolve/{}/{}",
        asset_ref.origin_runtime_id, asset_ref.sha256
    );
    let expected_size = asset_ref.size;
    let handle = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            if request.starts_with("GET /workspaces ") {
                let body = r#"{"workspaces":[{"slug":"room"}]}"#;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            } else {
                assert!(request.starts_with(&format!("GET {expected_path}?")));
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {expected_size}\r\nConnection: close\r\n\r\npartial"
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            }
            socket.shutdown().await.unwrap();
        }
    });
    (Client::new(format!("http://{addr}")), handle)
}

async fn spawn_raw_asset_server(
    asset_ref: &AssetRef,
    asset_response: Vec<u8>,
) -> (Client, SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let expected_path = format!(
        "/workspaces/room/assets/resolve/{}/{}",
        asset_ref.origin_runtime_id, asset_ref.sha256
    );
    let handle = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 1024];
            while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                let read = socket.read(&mut buffer).await.unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let request = String::from_utf8_lossy(&request);
            if request.starts_with("GET /workspaces ") {
                let body = r#"{"workspaces":[{"slug":"room"}]}"#;
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await
                    .unwrap();
            } else {
                assert!(request.starts_with(&format!("GET {expected_path}?")));
                socket.write_all(&asset_response).await.unwrap();
            }
            socket.shutdown().await.unwrap();
        }
    });
    (Client::new(format!("http://{addr}")), addr, handle)
}

async fn assert_invalid_length_preserves_destination(asset_ref: &AssetRef, response: Vec<u8>) {
    let (client, _, server) = spawn_raw_asset_server(asset_ref, response).await;
    let output_dir = tempfile::tempdir().unwrap();
    let destination = output_dir.path().join("report.bin");
    write(&destination, b"original");
    let error = cmd_asset::get(
        &client,
        cmd_asset::GetArgs {
            workspace: None,
            asset_ref: asset_ref.to_string(),
            output: Some(destination.clone()),
            force: true,
        },
    )
    .await
    .expect_err("invalid Content-Length must fail");
    assert!(matches!(
        error,
        CliError::InvalidConfig(_) | CliError::Transport(_)
    ));
    assert_eq!(gitim_runtime::cli::from_cli_error(&error), 1);
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 1);
    server.abort();
}

#[tokio::test]
async fn get_requires_one_exact_content_length_before_staging() {
    let expected = b"expected";
    let asset_ref = reference_for(expected, "report.bin");
    let missing = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n8\r\nexpected\r\n0\r\n\r\n".to_vec();
    assert_invalid_length_preserves_destination(&asset_ref, missing).await;

    let duplicate = b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nContent-Length: 8\r\nConnection: close\r\n\r\nexpected".to_vec();
    assert_invalid_length_preserves_destination(&asset_ref, duplicate).await;

    let invalid =
        b"HTTP/1.1 200 OK\r\nContent-Length: nope\r\nConnection: close\r\n\r\nexpected".to_vec();
    assert_invalid_length_preserves_destination(&asset_ref, invalid).await;

    let mismatch =
        b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nexpecte".to_vec();
    assert_invalid_length_preserves_destination(&asset_ref, mismatch).await;

    let oversize = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        gitim_core::types::MAX_ASSET_BYTES + 1
    )
    .into_bytes();
    assert_invalid_length_preserves_destination(&asset_ref, oversize).await;
}

#[tokio::test]
async fn hash_mismatch_removes_temp_and_preserves_existing_destination() {
    let expected = b"expected";
    let asset_ref = reference_for(expected, "report.bin");
    let response = (StatusCode::OK, Body::from("tampered")).into_response();
    let (client, server) = spawn_download_server(&asset_ref, response).await;
    let output_dir = tempfile::tempdir().unwrap();
    let destination = output_dir.path().join("report.bin");
    write(&destination, b"original");

    let error = cmd_asset::get(
        &client,
        cmd_asset::GetArgs {
            workspace: None,
            asset_ref: asset_ref.to_string(),
            output: Some(destination.clone()),
            force: true,
        },
    )
    .await
    .expect_err("hash mismatch must fail");
    assert!(matches!(error, CliError::InvalidConfig(_)));
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 1);
    server.abort();
}

#[tokio::test]
async fn transport_abort_leaves_no_destination_or_partial_file() {
    let expected = b"complete bytes";
    let asset_ref = reference_for(expected, "report.bin");
    let (client, server) = spawn_aborting_download_server(&asset_ref).await;
    let output_dir = tempfile::tempdir().unwrap();
    let destination = output_dir.path().join("report.bin");

    let error = cmd_asset::get(
        &client,
        cmd_asset::GetArgs {
            workspace: Some("room".to_string()),
            asset_ref: asset_ref.to_string(),
            output: Some(destination.clone()),
            force: false,
        },
    )
    .await
    .expect_err("aborted body must fail");
    assert!(matches!(error, CliError::Transport(_)), "{error:?}");
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 0);
    server.abort();
}

#[tokio::test]
async fn binary_non_success_uses_structured_error_classification() {
    let expected = b"bytes";
    let asset_ref = reference_for(expected, "report.bin");
    let response = (
        StatusCode::NOT_FOUND,
        Json(json!({
            "ok": false,
            "error": "asset unavailable",
            "error_code": "asset_not_found"
        })),
    )
        .into_response();
    let (client, server) = spawn_download_server(&asset_ref, response).await;
    let output_dir = tempfile::tempdir().unwrap();

    let error = cmd_asset::get(
        &client,
        cmd_asset::GetArgs {
            workspace: None,
            asset_ref: asset_ref.to_string(),
            output: Some(output_dir.path().join("report.bin")),
            force: false,
        },
    )
    .await
    .expect_err("structured error must fail");
    assert!(matches!(
        error,
        CliError::ResponseErrorCode { ref code, .. } if code == "asset_not_found"
    ));
    assert_eq!(std::fs::read_dir(output_dir.path()).unwrap().count(), 0);
    server.abort();
}

#[tokio::test]
async fn successful_binary_response_is_not_buffered_by_client() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = [0_u8; 1024];
        let _ = socket.read(&mut request).await.unwrap();
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 52428800\r\nConnection: close\r\n\r\nprefix",
            )
            .await
            .unwrap();
        std::future::pending::<()>().await;
    });
    let client = Client::new(format!("http://{addr}"));

    let response = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        client.get_binary("/asset"),
    )
    .await
    .expect("get_binary returns after headers")
    .expect("200 response");
    assert_eq!(response.status(), StatusCode::OK);
    drop(response);
    server.abort();
}

async fn spawn_multipart_capture_server() -> (
    Client,
    tokio::sync::oneshot::Receiver<(String, Vec<u8>)>,
    tokio::task::JoinHandle<()>,
) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut received = Vec::new();
        let mut buffer = [0_u8; 8192];
        let header_end = loop {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "request closed before headers");
            received.extend_from_slice(&buffer[..read]);
            if let Some(index) = received.windows(4).position(|window| window == b"\r\n\r\n") {
                break index + 4;
            }
        };
        let headers = String::from_utf8(received[..header_end].to_vec()).unwrap();
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length: ")
                    .map(str::to_string)
            })
            .expect("multipart has aggregate Content-Length")
            .parse::<usize>()
            .unwrap();
        while received.len() - header_end < content_length {
            let read = socket.read(&mut buffer).await.unwrap();
            assert!(read > 0, "multipart body closed early");
            received.extend_from_slice(&buffer[..read]);
        }
        let body = received[header_end..header_end + content_length].to_vec();
        sender.send((headers, body)).unwrap();
        let response = b"{\"ok\":true}";
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    response.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        socket.write_all(response).await.unwrap();
        socket.shutdown().await.unwrap();
    });
    (Client::new(format!("http://{addr}")), receiver, server)
}

#[tokio::test]
async fn multipart_has_aggregate_length_order_and_opened_handle_bound() {
    let files = tempfile::tempdir().unwrap();
    let first_path = files.path().join("first.bin");
    let second_path = files.path().join("second.bin");
    write(&first_path, b"abc");
    write(&second_path, b"XY");
    let first = tokio::fs::File::open(&first_path).await.unwrap();
    let second = tokio::fs::File::open(&second_path).await.unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(&first_path)
        .unwrap()
        .write_all(b"grown")
        .unwrap();
    let uploads = vec![
        UploadFile {
            file: first,
            file_name: "first.bin".to_string(),
            length: 3,
        },
        UploadFile {
            file: second,
            file_name: "second.bin".to_string(),
            length: 2,
        },
    ];
    let (client, capture, server) = spawn_multipart_capture_server().await;
    client
        .post_files("/upload", uploads)
        .await
        .expect("bounded multipart succeeds");
    let (headers, body) = capture.await.unwrap();
    let declared = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .map(str::to_string)
        })
        .unwrap()
        .parse::<usize>()
        .unwrap();
    assert_eq!(declared, body.len());
    let body = String::from_utf8(body).unwrap();
    let first_index = body.find("filename=\"first.bin\"").unwrap();
    let second_index = body.find("filename=\"second.bin\"").unwrap();
    assert!(first_index < second_index);
    assert!(body.contains("\r\n\r\nabc\r\n"));
    assert!(!body.contains("abcgrown"));
    assert!(body.contains("\r\n\r\nXY\r\n"));
    assert!(body.ends_with("--\r\n"));
    server.await.unwrap();
}

#[tokio::test]
async fn multipart_short_read_is_a_transport_failure() {
    let files = tempfile::tempdir().unwrap();
    let path = files.path().join("shrunk.bin");
    write(&path, b"abc");
    let file = tokio::fs::File::open(&path).await.unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(1)
        .unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buffer = [0_u8; 4096];
        while socket.read(&mut buffer).await.unwrap_or(0) > 0 {}
    });
    let client = Client::new(format!("http://{addr}"));
    let result = tokio::time::timeout(
        Duration::from_secs(2),
        client.post_files(
            "/upload",
            vec![UploadFile {
                file,
                file_name: "shrunk.bin".to_string(),
                length: 3,
            }],
        ),
    )
    .await
    .expect("short multipart fails without hanging")
    .expect_err("short multipart must fail");
    assert!(matches!(result, CliError::Transport(_)), "{result:?}");
    server.await.unwrap();
}

async fn spawn_cli_error_server(
    asset_ref: &AssetRef,
    response: Vec<u8>,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let (_, addr, server) = spawn_raw_asset_server(asset_ref, response).await;
    (addr, server)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn binary_asset_get_preserves_exit_code_classes() {
    let asset_ref = reference_for(b"bytes", "report.bin");
    let structured_body = r#"{"ok":false,"error":"missing","error_code":"asset_not_found"}"#;
    let structured = format!(
        "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{structured_body}",
        structured_body.len()
    )
    .into_bytes();
    let (addr, server) = spawn_cli_error_server(&asset_ref, structured).await;
    let output_dir = tempfile::tempdir().unwrap();
    let output = run_cli(
        addr,
        output_dir.path().to_path_buf(),
        vec![
            "asset".into(),
            "get".into(),
            "--ref".into(),
            asset_ref.to_string(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(2));
    server.await.unwrap();

    let unstructured =
        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops"
            .to_vec();
    let (addr, server) = spawn_cli_error_server(&asset_ref, unstructured).await;
    let output = run_cli(
        addr,
        output_dir.path().to_path_buf(),
        vec![
            "asset".into(),
            "get".into(),
            "--ref".into(),
            asset_ref.to_string(),
        ],
    )
    .await;
    assert_eq!(output.status.code(), Some(3));
    server.await.unwrap();
}
