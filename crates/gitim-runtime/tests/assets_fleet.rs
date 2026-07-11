#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use axum::body::{Body, Bytes};
use axum::extract::{Path as AxumPath, State};
use axum::http::{header, HeaderMap, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use futures::StreamExt;
#[cfg(feature = "test-support")]
use gitim_runtime::assets::{resolve_fleet_asset_for_test, AssetEvent};
use gitim_runtime::assets::{AssetLimits, AssetService, AssetSource};
use gitim_runtime::fleet;
use gitim_runtime::git_config::{GitConfig, GitProvider, WorkspaceConfig};
use gitim_runtime::http::{create_router, SharedRuntimeState};
use gitim_runtime::user_config::{FleetNodeEntry, FleetWorkspaceMapping};
use gitim_runtime::workspace::WorkspaceContext;
use http_body_util::BodyExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::convert::Infallible;
use std::path::{Path, PathBuf};
#[cfg(feature = "test-support")]
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
#[cfg(feature = "test-support")]
use std::sync::Barrier;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::Notify;
#[cfg(feature = "test-support")]
use tokio::sync::Semaphore;
use tower::ServiceExt;

mod common;
use common::HomeGuard;
use serial_test::serial;

const LOCAL_RUNTIME_ID: &str = "24a6489c-762e-4461-9247-a824807a6080";
const ORIGIN_RUNTIME_ID: &str = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const FALLBACK_RUNTIME_ID: &str = "8bd33162-3f61-497f-9ef9-f237979a9cca";
const CREATED_AT: &str = "2026-07-11T00:00:00Z";
const WORKSPACE_IDENTITY: &str = "github.com/acme/room";
const PNG_1X1: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, b'I', b'H', b'D', b'R',
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, b'I', b'D', b'A', b'T', 0x08, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
    0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, b'I', b'E',
    b'N', b'D', 0xae, 0x42, 0x60, 0x82,
];

#[derive(Clone)]
enum PeerBehavior {
    Object(Vec<u8>),
    HeadMissingObject(Vec<u8>),
    Missing,
    Wrong(Vec<u8>),
    Malformed,
    IncompleteMetadata(Vec<u8>),
    Oversized(u64),
    HeaderDelay {
        bytes: Vec<u8>,
        delay: Duration,
    },
    StalledBody {
        declared_size: u64,
    },
    SlowChunks {
        declared_size: u64,
        interval: Duration,
    },
    #[cfg(feature = "test-support")]
    StalledHead,
}

struct TransferGuard {
    state: Arc<MockPeerState>,
}

struct HeadGuard {
    state: Arc<MockPeerState>,
}

impl Drop for HeadGuard {
    fn drop(&mut self) {
        self.state.inflight_heads.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for TransferGuard {
    fn drop(&mut self) {
        self.state.inflight_gets.fetch_sub(1, Ordering::AcqRel);
    }
}

struct MockPeerState {
    runtime_id: Option<String>,
    behavior: PeerBehavior,
    health_requests: AtomicUsize,
    object_gets: AtomicUsize,
    object_heads: AtomicUsize,
    inflight_gets: AtomicUsize,
    max_inflight_gets: AtomicUsize,
    inflight_heads: AtomicUsize,
    max_inflight_heads: AtomicUsize,
    browser_headers_seen: AtomicBool,
    observed_slug: Mutex<Option<String>>,
    get_entered: Notify,
    #[cfg(feature = "test-support")]
    head_release: Semaphore,
}

struct MockPeer {
    base_url: String,
    state: Arc<MockPeerState>,
    task: tokio::task::JoinHandle<()>,
}

struct PathAwareStoreState {
    requests: Mutex<Vec<String>>,
}

struct PathAwareStorePeer {
    base_url: String,
    state: Arc<PathAwareStoreState>,
    task: tokio::task::JoinHandle<()>,
}

struct StalledHealthState {
    requests: AtomicUsize,
    inflight: AtomicUsize,
    released: AtomicBool,
    release: Notify,
    hold_objects: bool,
    object_requests: AtomicUsize,
    object_inflight: AtomicUsize,
    objects_released: AtomicBool,
    object_release: Notify,
}

struct StalledHealthGuard {
    state: Arc<StalledHealthState>,
}

struct StalledObjectGuard {
    state: Arc<StalledHealthState>,
}

impl Drop for StalledHealthGuard {
    fn drop(&mut self) {
        self.state.inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Drop for StalledObjectGuard {
    fn drop(&mut self) {
        self.state.object_inflight.fetch_sub(1, Ordering::AcqRel);
    }
}

struct StalledHealthPeer {
    base_url: String,
    state: Arc<StalledHealthState>,
    task: tokio::task::JoinHandle<()>,
}

impl StalledHealthPeer {
    async fn spawn() -> Self {
        Self::spawn_with_object_hold(false).await
    }

    async fn spawn_holding_objects() -> Self {
        Self::spawn_with_object_hold(true).await
    }

    async fn spawn_with_object_hold(hold_objects: bool) -> Self {
        let state = Arc::new(StalledHealthState {
            requests: AtomicUsize::new(0),
            inflight: AtomicUsize::new(0),
            released: AtomicBool::new(false),
            release: Notify::new(),
            hold_objects,
            object_requests: AtomicUsize::new(0),
            object_inflight: AtomicUsize::new(0),
            objects_released: AtomicBool::new(false),
            object_release: Notify::new(),
        });
        let app = Router::new()
            .route("/health", get(stalled_health))
            .route(
                "/workspaces/{slug}/assets/objects/{hash}",
                get(stalled_object).head(stalled_object),
            )
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }

    fn requests(&self) -> usize {
        self.state.requests.load(Ordering::Acquire)
    }

    fn inflight(&self) -> usize {
        self.state.inflight.load(Ordering::Acquire)
    }

    fn release(&self) {
        self.state.released.store(true, Ordering::Release);
        self.state.release.notify_waiters();
    }

    async fn wait_until_idle(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut stable_rounds = 0;
        let mut observed_requests = self.requests();
        while std::time::Instant::now() < deadline {
            let requests = self.requests();
            if self.inflight() == 0 && requests == observed_requests {
                stable_rounds += 1;
                if stable_rounds == 100 {
                    return;
                }
            } else {
                stable_rounds = 0;
                observed_requests = requests;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(self.inflight(), 0);
    }

    fn object_requests(&self) -> usize {
        self.state.object_requests.load(Ordering::Acquire)
    }

    fn object_inflight(&self) -> usize {
        self.state.object_inflight.load(Ordering::Acquire)
    }

    fn release_objects(&self) {
        self.state.objects_released.store(true, Ordering::Release);
        self.state.object_release.notify_waiters();
    }

    async fn wait_for_objects_to_finish(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.object_inflight() != 0 && std::time::Instant::now() < deadline {
            tokio::task::yield_now().await;
        }
        assert_eq!(self.object_inflight(), 0);
    }
}

impl Drop for StalledHealthPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn stalled_health(State(state): State<Arc<StalledHealthState>>) -> Response {
    state.requests.fetch_add(1, Ordering::AcqRel);
    state.inflight.fetch_add(1, Ordering::AcqRel);
    let _guard = StalledHealthGuard { state };
    while !_guard.state.released.load(Ordering::Acquire) {
        _guard.state.release.notified().await;
    }
    Response::builder()
        .status(StatusCode::SERVICE_UNAVAILABLE)
        .body(Body::empty())
        .unwrap()
}

async fn stalled_object(State(state): State<Arc<StalledHealthState>>) -> Response {
    state.object_requests.fetch_add(1, Ordering::AcqRel);
    state.object_inflight.fetch_add(1, Ordering::AcqRel);
    let _guard = StalledObjectGuard { state };
    while _guard.state.hold_objects && !_guard.state.objects_released.load(Ordering::Acquire) {
        _guard.state.object_release.notified().await;
    }
    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::empty())
        .unwrap()
}

impl MockPeer {
    async fn spawn(runtime_id: &str, behavior: PeerBehavior) -> Self {
        Self::spawn_with_runtime_id(Some(runtime_id), behavior).await
    }

    async fn spawn_unresolved(behavior: PeerBehavior) -> Self {
        Self::spawn_with_runtime_id(None, behavior).await
    }

    async fn spawn_with_runtime_id(runtime_id: Option<&str>, behavior: PeerBehavior) -> Self {
        let state = Arc::new(MockPeerState {
            runtime_id: runtime_id.map(str::to_string),
            behavior,
            health_requests: AtomicUsize::new(0),
            object_gets: AtomicUsize::new(0),
            object_heads: AtomicUsize::new(0),
            inflight_gets: AtomicUsize::new(0),
            max_inflight_gets: AtomicUsize::new(0),
            inflight_heads: AtomicUsize::new(0),
            max_inflight_heads: AtomicUsize::new(0),
            browser_headers_seen: AtomicBool::new(false),
            observed_slug: Mutex::new(None),
            get_entered: Notify::new(),
            #[cfg(feature = "test-support")]
            head_release: Semaphore::new(0),
        });
        let app = Router::new()
            .route("/health", get(peer_health))
            .route(
                "/workspaces/{slug}/assets/objects/{hash}",
                get(peer_object).head(peer_object),
            )
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }

    fn object_gets(&self) -> usize {
        self.state.object_gets.load(Ordering::Acquire)
    }

    fn object_heads(&self) -> usize {
        self.state.object_heads.load(Ordering::Acquire)
    }

    fn health_requests(&self) -> usize {
        self.state.health_requests.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-support")]
    fn inflight_heads(&self) -> usize {
        self.state.inflight_heads.load(Ordering::Acquire)
    }

    #[cfg(feature = "test-support")]
    fn release_heads(&self) {
        self.state.head_release.add_permits(self.inflight_heads());
    }

    #[cfg(feature = "test-support")]
    async fn wait_for_heads_to_finish(&self) {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.inflight_heads() != 0 && std::time::Instant::now() < deadline {
            tokio::task::yield_now().await;
        }
        assert_eq!(self.inflight_heads(), 0);
    }

    fn max_inflight_gets(&self) -> usize {
        self.state.max_inflight_gets.load(Ordering::Acquire)
    }

    fn max_inflight_heads(&self) -> usize {
        self.state.max_inflight_heads.load(Ordering::Acquire)
    }

    fn browser_headers_seen(&self) -> bool {
        self.state.browser_headers_seen.load(Ordering::Acquire)
    }

    fn observed_slug(&self) -> Option<String> {
        self.state.observed_slug.lock().unwrap().clone()
    }

    fn shutdown(&self) {
        self.task.abort();
    }
}

impl Drop for MockPeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl PathAwareStorePeer {
    async fn spawn() -> Self {
        let state = Arc::new(PathAwareStoreState {
            requests: Mutex::new(Vec::new()),
        });
        let app = Router::new()
            .route("/health", get(path_aware_store_health))
            .route(
                "/workspaces/{slug}/assets/objects/{hash}",
                get(path_aware_store_object),
            )
            .with_state(Arc::clone(&state));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}"),
            state,
            task,
        }
    }

    fn requests(&self) -> Vec<String> {
        self.state.requests.lock().unwrap().clone()
    }
}

impl Drop for PathAwareStorePeer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn path_aware_store_health() -> axum::Json<Value> {
    axum::Json(serde_json::json!({
        "service": "gitim-runtime",
        "runtime_id": ORIGIN_RUNTIME_ID,
    }))
}

async fn path_aware_store_object(
    State(state): State<Arc<PathAwareStoreState>>,
    AxumPath((slug, hash)): AxumPath<(String, String)>,
) -> Response {
    state
        .requests
        .lock()
        .unwrap()
        .push(format!("/workspaces/{slug}/assets/objects/{hash}"));
    if slug == "store-b" {
        object_response(&hash, PNG_1X1.to_vec(), false)
    } else {
        Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap()
    }
}

async fn peer_health(State(state): State<Arc<MockPeerState>>) -> axum::Json<Value> {
    state.health_requests.fetch_add(1, Ordering::AcqRel);
    axum::Json(serde_json::json!({
        "service": "gitim-runtime",
        "runtime_id": state.runtime_id,
    }))
}

async fn peer_object(
    State(state): State<Arc<MockPeerState>>,
    method: Method,
    headers: HeaderMap,
    AxumPath((slug, hash)): AxumPath<(String, String)>,
) -> Response {
    *state.observed_slug.lock().unwrap() = Some(slug);
    if headers.contains_key(header::ORIGIN)
        || [
            "sec-fetch-site",
            "sec-fetch-mode",
            "sec-fetch-dest",
            "sec-fetch-user",
        ]
        .iter()
        .any(|name| headers.contains_key(*name))
    {
        state.browser_headers_seen.store(true, Ordering::Release);
    }
    if method == Method::HEAD {
        state.object_heads.fetch_add(1, Ordering::AcqRel);
        let inflight = state.inflight_heads.fetch_add(1, Ordering::AcqRel) + 1;
        state
            .max_inflight_heads
            .fetch_max(inflight, Ordering::AcqRel);
    } else {
        state.object_gets.fetch_add(1, Ordering::AcqRel);
        let inflight = state.inflight_gets.fetch_add(1, Ordering::AcqRel) + 1;
        state
            .max_inflight_gets
            .fetch_max(inflight, Ordering::AcqRel);
        state.get_entered.notify_one();
    }
    let _guard = (method == Method::GET).then(|| TransferGuard {
        state: Arc::clone(&state),
    });
    let _head_guard = (method == Method::HEAD).then(|| HeadGuard {
        state: Arc::clone(&state),
    });
    match state.behavior.clone() {
        PeerBehavior::Missing => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
        PeerBehavior::Malformed => Response::builder()
            .status(StatusCode::OK)
            .body(Body::from("malformed"))
            .unwrap(),
        PeerBehavior::IncompleteMetadata(bytes) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, bytes.len().to_string())
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::ETAG, format!("\"sha256-{hash}\""))
            .header("x-content-type-options", "nosniff")
            .body(if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from(bytes)
            })
            .unwrap(),
        PeerBehavior::Oversized(length) => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, length.to_string())
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::ETAG, format!("\"sha256-{hash}\""))
            .body(Body::empty())
            .unwrap(),
        PeerBehavior::Object(bytes) | PeerBehavior::Wrong(bytes) => {
            object_response(&hash, bytes, method == Method::HEAD)
        }
        PeerBehavior::HeadMissingObject(bytes) => {
            if method == Method::HEAD {
                Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(Body::empty())
                    .unwrap()
            } else {
                object_response(&hash, bytes, false)
            }
        }
        PeerBehavior::HeaderDelay { bytes, delay } => {
            tokio::time::sleep(delay).await;
            object_response(&hash, bytes, method == Method::HEAD)
        }
        PeerBehavior::StalledBody { declared_size } => Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_LENGTH, declared_size.to_string())
            .header(header::CONTENT_TYPE, "application/octet-stream")
            .header(header::ETAG, format!("\"sha256-{hash}\""))
            .header("x-content-type-options", "nosniff")
            .header(
                header::CACHE_CONTROL,
                "private, immutable, max-age=31536000",
            )
            .header(header::ACCEPT_RANGES, "bytes")
            .body(if method == Method::HEAD {
                Body::empty()
            } else {
                Body::from_stream(
                    futures::stream::once(async {
                        Ok::<Bytes, Infallible>(Bytes::from_static(b"x"))
                    })
                    .chain(futures::stream::pending::<Result<Bytes, Infallible>>()),
                )
            })
            .unwrap(),
        PeerBehavior::SlowChunks {
            declared_size,
            interval,
        } => {
            let stream = futures::stream::unfold(0_u64, move |sent| async move {
                if sent >= declared_size {
                    return None;
                }
                tokio::time::sleep(interval).await;
                Some((Ok::<Bytes, Infallible>(Bytes::from_static(b"x")), sent + 1))
            });
            Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_LENGTH, declared_size.to_string())
                .header(header::CONTENT_TYPE, "application/octet-stream")
                .header(header::ETAG, format!("\"sha256-{hash}\""))
                .header("x-content-type-options", "nosniff")
                .header(
                    header::CACHE_CONTROL,
                    "private, immutable, max-age=31536000",
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .body(if method == Method::HEAD {
                    Body::empty()
                } else {
                    Body::from_stream(stream)
                })
                .unwrap()
        }
        #[cfg(feature = "test-support")]
        PeerBehavior::StalledHead => {
            if method == Method::HEAD {
                state.head_release.acquire().await.unwrap().forget();
            }
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap()
        }
    }
}

fn object_response(hash: &str, bytes: Vec<u8>, head_only: bool) -> Response {
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_LENGTH, bytes.len().to_string())
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::ETAG, format!("\"sha256-{hash}\""))
        .header("x-content-type-options", "nosniff")
        .header(
            header::CACHE_CONTROL,
            "private, immutable, max-age=31536000",
        )
        .header(header::ACCEPT_RANGES, "bytes")
        .body(if head_only {
            Body::empty()
        } else {
            Body::from(bytes)
        })
        .unwrap()
}

struct Fixture {
    app: Router,
    state: SharedRuntimeState,
    workspace: TempDir,
    service: Arc<AssetService>,
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
    let workspace_path = workspace.path().canonicalize().unwrap();
    let (app, state) = create_router();
    let mut context = WorkspaceContext::new(
        "room".to_string(),
        "Room".to_string(),
        workspace_path.clone(),
    );
    context.git_config = Some(WorkspaceConfig {
        workspace: workspace_path.to_string_lossy().into_owned(),
        created_at: CREATED_AT.to_string(),
        git: GitConfig {
            provider: GitProvider::Github,
            remote_url: Some("https://github.com/acme/room.git".to_string()),
            token: None,
            github_email: None,
        },
    });
    let service = Arc::new(AssetService::new(limits));
    service
        .activate_workspace(
            &workspace_path,
            format!("github:{WORKSPACE_IDENTITY}"),
            &context.asset_token,
        )
        .unwrap();
    {
        let mut runtime = state.lock().unwrap();
        runtime.runtime_id = LOCAL_RUNTIME_ID.to_string();
        runtime.assets = Arc::clone(&service);
        runtime.workspaces.insert("room".to_string(), context);
    }
    Fixture {
        app,
        state,
        workspace,
        service,
    }
}

fn fixture() -> Fixture {
    fixture_with_limits(limits())
}

fn add_peer(
    fixture: &Fixture,
    alias: &str,
    peer: &MockPeer,
    runtime_id: Option<&str>,
    workspace_identity: &str,
) {
    add_peer_url(
        fixture,
        alias,
        &peer.base_url,
        runtime_id,
        workspace_identity,
    );
}

fn add_peer_url(
    fixture: &Fixture,
    alias: &str,
    base_url: &str,
    runtime_id: Option<&str>,
    workspace_identity: &str,
) {
    add_peer_mapping_url(
        fixture,
        alias,
        base_url,
        runtime_id,
        workspace_identity,
        "remote-room",
    );
}

fn add_peer_mapping_url(
    fixture: &Fixture,
    alias: &str,
    base_url: &str,
    runtime_id: Option<&str>,
    workspace_identity: &str,
    remote_workspace_id: &str,
) {
    add_peer_mapping_url_to_state(
        &fixture.state,
        alias,
        base_url,
        runtime_id,
        workspace_identity,
        remote_workspace_id,
    );
}

fn add_peer_mapping_url_to_state(
    state: &SharedRuntimeState,
    alias: &str,
    base_url: &str,
    runtime_id: Option<&str>,
    workspace_identity: &str,
    remote_workspace_id: &str,
) {
    fleet::activate_node(
        state.clone(),
        FleetNodeEntry {
            node_id: alias.to_string(),
            runtime_id: runtime_id.map(str::to_string),
            base_url: base_url.to_string(),
            node_ip: None,
            node_name: None,
            workspaces: vec![remote_workspace_id.to_string()],
            workspace_mappings: vec![FleetWorkspaceMapping {
                remote_workspace_id: remote_workspace_id.to_string(),
                local_workspace_id: "room".to_string(),
                workspace_identity: workspace_identity.to_string(),
            }],
            ssh_tunnel: None,
        },
    );
}

async fn spawn_raw_truncated_peer(hash: String) -> (String, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let hash = hash.clone();
            tokio::spawn(async move {
                let mut request = Vec::new();
                let mut chunk = [0_u8; 1024];
                while !request.windows(4).any(|window| window == b"\r\n\r\n") {
                    let Ok(read) = socket.read(&mut chunk).await else {
                        return;
                    };
                    if read == 0 || request.len() > 16 * 1024 {
                        return;
                    }
                    request.extend_from_slice(&chunk[..read]);
                }
                let request = String::from_utf8_lossy(&request);
                let first_line = request.lines().next().unwrap_or_default();
                if !first_line.contains("/assets/objects/") {
                    let _ = socket
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
                let headers = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: 20\r\nContent-Type: application/octet-stream\r\nETag: \"sha256-{hash}\"\r\nX-Content-Type-Options: nosniff\r\nCache-Control: private, immutable, max-age=31536000\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n"
                );
                if socket.write_all(headers.as_bytes()).await.is_err() {
                    return;
                }
                if first_line.starts_with("GET ") {
                    let _ = socket.write_all(b"short").await;
                }
                let _ = socket.shutdown().await;
            });
        }
    });
    (format!("http://{address}"), task)
}

async fn spawn_accept_close_peer() -> (String, Arc<AtomicUsize>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let accepts = Arc::new(AtomicUsize::new(0));
    let task_accepts = Arc::clone(&accepts);
    let task = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                return;
            };
            task_accepts.fetch_add(1, Ordering::AcqRel);
            drop(socket);
        }
    });
    (format!("http://{address}"), accepts, task)
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn resolve_request(method: Method, origin: &str, hash: &str) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(format!(
            "/workspaces/room/assets/resolve/{origin}/{hash}?name=pixel.png"
        ))
        .body(Body::empty())
        .unwrap()
}

async fn response_bytes(response: Response) -> Bytes {
    response.into_body().collect().await.unwrap().to_bytes()
}

async fn response_json(response: Response) -> Value {
    serde_json::from_slice(&response_bytes(response).await).unwrap()
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

fn temp_files(workspace: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(workspace.join(".gitim-runtime/assets/v1/tmp"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
}

fn workspace_store(fixture: &Fixture) -> gitim_runtime::assets::AssetStore {
    let token = fixture
        .state
        .lock()
        .unwrap()
        .workspaces
        .get("room")
        .unwrap()
        .asset_token
        .clone();
    fixture
        .service
        .open_registered_store(
            fixture.workspace.path(),
            &format!("github:{WORKSPACE_IDENTITY}"),
            &token,
        )
        .unwrap()
}

#[cfg(feature = "test-support")]
fn spawn_resolver_child(
    workspace: &Path,
    base_url: &str,
    hash: &str,
    start: &Path,
    ready: &Path,
) -> Child {
    Command::new(std::env::current_exe().unwrap())
        .arg("--exact")
        .arg("fleet_resolver_child")
        .arg("--ignored")
        .arg("--nocapture")
        .env("GITIM_FLEET_RESOLVER_CHILD", "1")
        .env("GITIM_FLEET_RESOLVER_WORKSPACE", workspace)
        .env("GITIM_FLEET_RESOLVER_BASE_URL", base_url)
        .env("GITIM_FLEET_RESOLVER_HASH", hash)
        .env("GITIM_FLEET_RESOLVER_START", start)
        .env("GITIM_FLEET_RESOLVER_READY", ready)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap()
}

#[cfg(feature = "test-support")]
fn wait_for_child_marker(child: &mut Child, marker: &Path) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() {
        assert!(child.try_wait().unwrap().is_none());
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn remote_get_verifies_persists_and_survives_origin_shutdown() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let hash = sha256(PNG_1X1);

    let first = fixture
        .app
        .clone()
        .clone()
        .oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();

    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(response_bytes(first).await.as_ref(), PNG_1X1);
    assert_eq!(peer.object_gets(), 1);
    assert_eq!(peer.observed_slug().as_deref(), Some("remote-room"));
    assert_eq!(object_files(fixture.workspace.path()).len(), 1);
    assert!(!peer.browser_headers_seen());
    let metadata = workspace_store(&fixture).inspect(&hash).unwrap();
    assert_eq!(
        metadata.source,
        AssetSource::FleetReplica {
            origin_runtime_id: ORIGIN_RUNTIME_ID.to_string(),
        }
    );
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        0
    );

    peer.shutdown();
    let second = fixture
        .app
        .clone()
        .oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(response_bytes(second).await.as_ref(), PNG_1X1);
}

#[tokio::test]
async fn exact_origin_tries_distinct_remote_stores_and_deduplicates_identical_endpoints() {
    let peer = PathAwareStorePeer::spawn().await;
    let fixture = fixture();
    fleet::activate_node(
        fixture.state.clone(),
        FleetNodeEntry {
            node_id: "origin".to_string(),
            runtime_id: Some(ORIGIN_RUNTIME_ID.to_string()),
            base_url: peer.base_url.clone(),
            node_ip: None,
            node_name: None,
            workspaces: vec!["store-a".to_string(), "store-b".to_string()],
            workspace_mappings: vec![
                FleetWorkspaceMapping {
                    remote_workspace_id: "store-a".to_string(),
                    local_workspace_id: "room".to_string(),
                    workspace_identity: WORKSPACE_IDENTITY.to_string(),
                },
                FleetWorkspaceMapping {
                    remote_workspace_id: "store-a".to_string(),
                    local_workspace_id: "room".to_string(),
                    workspace_identity: WORKSPACE_IDENTITY.to_string(),
                },
                FleetWorkspaceMapping {
                    remote_workspace_id: "store-b".to_string(),
                    local_workspace_id: "room".to_string(),
                    workspace_identity: WORKSPACE_IDENTITY.to_string(),
                },
            ],
            ssh_tunnel: None,
        },
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await.as_ref(), PNG_1X1);
    let hash = sha256(PNG_1X1);
    assert_eq!(
        peer.requests(),
        vec![
            format!("/workspaces/store-a/assets/objects/{hash}"),
            format!("/workspaces/store-b/assets/objects/{hash}"),
        ]
    );
}

#[tokio::test]
async fn fallback_finds_replica_after_third_sorted_alias() {
    let origin = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Missing).await;
    let alpha = MockPeer::spawn(
        "11111111-1111-4111-8111-111111111111",
        PeerBehavior::Missing,
    )
    .await;
    let beta = MockPeer::spawn(
        "22222222-2222-4222-8222-222222222222",
        PeerBehavior::Missing,
    )
    .await;
    let gamma = MockPeer::spawn(
        "33333333-3333-4333-8333-333333333333",
        PeerBehavior::Missing,
    )
    .await;
    let omega = MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    for (alias, peer, runtime_id) in [
        ("origin", &origin, ORIGIN_RUNTIME_ID),
        ("alpha", &alpha, "11111111-1111-4111-8111-111111111111"),
        ("beta", &beta, "22222222-2222-4222-8222-222222222222"),
        ("gamma", &gamma, "33333333-3333-4333-8333-333333333333"),
        ("omega", &omega, FALLBACK_RUNTIME_ID),
    ] {
        add_peer(&fixture, alias, peer, Some(runtime_id), WORKSPACE_IDENTITY);
    }

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(omega.object_gets(), 1);
    assert_eq!(alpha.object_heads(), 1);
    assert_eq!(beta.object_heads(), 1);
    assert_eq!(gamma.object_heads(), 1);
    assert_eq!(omega.object_heads(), 1);
}

#[tokio::test]
async fn fallback_window_gets_earliest_positive_not_fastest_head() {
    let earlier = MockPeer::spawn(
        "11111111-1111-4111-8111-111111111111",
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_millis(75),
        },
    )
    .await;
    let later = MockPeer::spawn(
        "22222222-2222-4222-8222-222222222222",
        PeerBehavior::Object(PNG_1X1.to_vec()),
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "a-earlier",
        &earlier,
        Some("11111111-1111-4111-8111-111111111111"),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "b-later",
        &later,
        Some("22222222-2222-4222-8222-222222222222"),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(earlier.object_heads(), 1);
    assert_eq!(later.object_heads(), 1);
    assert_eq!(earlier.object_gets(), 1);
    assert_eq!(later.object_gets(), 0);
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn verified_fallback_resolves_before_large_legacy_discovery() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn().await;
    let verified =
        MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "verified",
        &verified,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    for index in 0..121 {
        add_peer_mapping_url(
            &fixture,
            &format!("legacy-{index:03}"),
            &stalled.base_url,
            None,
            WORKSPACE_IDENTITY,
            &format!("legacy-room-{index:03}"),
        );
    }
    let started = tokio::time::Instant::now();
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !request.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    let response = request.await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(verified.object_heads(), 1);
    assert_eq!(verified.object_gets(), 1);
    assert!(started.elapsed() < Duration::from_secs(10));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    stalled.release();
    stalled.wait_until_idle().await;
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn exact_origin_success_starts_bounded_legacy_backfill_without_delay() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn().await;
    let exact = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "exact",
        &exact,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    for index in 0..32 {
        add_peer_mapping_url(
            &fixture,
            &format!("legacy-{index:03}"),
            &stalled.base_url,
            None,
            WORKSPACE_IDENTITY,
            &format!("legacy-room-{index:03}"),
        );
    }
    let started = tokio::time::Instant::now();
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !request.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    let response = request.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await.as_ref(), PNG_1X1);
    assert_eq!(exact.object_gets(), 1);
    assert!(started.elapsed() < Duration::from_secs(8));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    assert!(stalled.requests() <= 8, "requests={}", stalled.requests());
    stalled.release();
    stalled.wait_until_idle().await;
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn concurrent_legacy_only_get_and_head_share_one_health_wave() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn().await;
    let fixture = fixture();
    for index in 0..32 {
        add_peer_mapping_url(
            &fixture,
            &format!("legacy-{index:03}"),
            &stalled.base_url,
            None,
            WORKSPACE_IDENTITY,
            &format!("legacy-room-{index:03}"),
        );
    }
    let get_app = fixture.app.clone();
    let get = tokio::spawn(async move {
        get_app
            .oneshot(resolve_request(
                Method::GET,
                ORIGIN_RUNTIME_ID,
                &sha256(b"get-missing"),
            ))
            .await
            .unwrap()
    });
    let head_app = fixture.app.clone();
    let head = tokio::spawn(async move {
        head_app
            .oneshot(resolve_request(
                Method::HEAD,
                ORIGIN_RUNTIME_ID,
                &sha256(b"head-missing"),
            ))
            .await
            .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while (!get.is_finished() || !head.is_finished())
        && stalled.requests() <= 8
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    assert!(stalled.requests() <= 8, "requests={}", stalled.requests());
    assert!(get.is_finished());
    assert!(head.is_finished());
    let get = get.await.unwrap();
    let head = head.await.unwrap();
    assert_eq!(get.status(), StatusCode::NOT_FOUND);
    assert_eq!(head.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    stalled.release();
    stalled.wait_until_idle().await;
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn legacy_object_missing_wins_after_background_health_timeout() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn_holding_objects().await;
    let fixture = fixture();
    add_peer_mapping_url(
        &fixture,
        "legacy",
        &stalled.base_url,
        None,
        WORKSPACE_IDENTITY,
        "remote-room",
    );
    let started = tokio::time::Instant::now();
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(b"missing"),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while (stalled.requests() == 0 || stalled.object_requests() == 0)
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    assert!(stalled.object_requests() > 0);
    tokio::time::advance(Duration::from_secs(8)).await;
    stalled.release_objects();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !request.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    let response = request.await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(started.elapsed() >= Duration::from_secs(8));
    assert!(started.elapsed() < Duration::from_secs(10));
    stalled.release();
    stalled.wait_until_idle().await;
    stalled.wait_for_objects_to_finish().await;
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn repeated_verified_fallback_heads_coalesce_legacy_backfill() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn().await;
    let verified =
        MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "verified",
        &verified,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    for index in 0..32 {
        add_peer_mapping_url(
            &fixture,
            &format!("legacy-{index:03}"),
            &stalled.base_url,
            None,
            WORKSPACE_IDENTITY,
            &format!("legacy-room-{index:03}"),
        );
    }
    let hash = sha256(PNG_1X1);
    let requests: Vec<_> = (0..16)
        .map(|_| {
            let app = fixture.app.clone();
            let hash = hash.clone();
            tokio::spawn(async move {
                app.oneshot(resolve_request(Method::HEAD, ORIGIN_RUNTIME_ID, &hash))
                    .await
                    .unwrap()
            })
        })
        .collect();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while requests.iter().any(|request| !request.is_finished())
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert!(requests.iter().all(tokio::task::JoinHandle::is_finished));
    let responses = futures::future::join_all(requests)
        .await
        .into_iter()
        .map(Result::unwrap)
        .collect::<Vec<_>>();
    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    assert!(stalled.requests() <= 8, "requests={}", stalled.requests());
    stalled.release();
    stalled.wait_until_idle().await;
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }

    let first_run_requests = stalled.requests();
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(Method::HEAD, ORIGIN_RUNTIME_ID, &hash))
            .await
            .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !request.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    let response = request.await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == first_run_requests && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > first_run_requests);
    stalled.release();
    stalled.wait_until_idle().await;
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
async fn legacy_only_resolution_does_not_wait_for_discovery_budget() {
    let _home = HomeGuard::install();
    let stalled = StalledHealthPeer::spawn().await;
    let fixture = fixture();
    for index in 0..121 {
        add_peer_mapping_url(
            &fixture,
            &format!("legacy-{index:03}"),
            &stalled.base_url,
            None,
            WORKSPACE_IDENTITY,
            &format!("legacy-room-{index:03}"),
        );
    }
    let started = tokio::time::Instant::now();
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while stalled.requests() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(stalled.requests() > 0);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !request.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    let response = request.await.unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(started.elapsed() < Duration::from_secs(8));
    assert!(stalled.requests() <= 8, "requests={}", stalled.requests());
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    stalled.release();
    stalled.wait_until_idle().await;
}

#[tokio::test]
#[serial(home_env)]
async fn unresolved_legacy_peer_remains_a_get_and_head_fallback() {
    let _home = HomeGuard::install();
    let peer = MockPeer::spawn_unresolved(PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(&fixture, "legacy", &peer, None, WORKSPACE_IDENTITY);
    let hash = sha256(PNG_1X1);

    let head = fixture
        .app
        .clone()
        .oneshot(resolve_request(Method::HEAD, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(response_bytes(head).await.is_empty());

    let get = fixture
        .app
        .oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::OK);
    assert_eq!(response_bytes(get).await.as_ref(), PNG_1X1);
    assert_eq!(peer.object_heads(), 2);
    assert_eq!(peer.object_gets(), 1);
    assert!(peer.health_requests() >= 1);
}

#[tokio::test]
#[serial(home_env)]
async fn verified_exact_origin_wins_endpoint_dedup_over_earlier_legacy_alias() {
    let _home = HomeGuard::install();
    let peer = MockPeer::spawn_unresolved(PeerBehavior::HeadMissingObject(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer_mapping_url(
        &fixture,
        "a-legacy",
        &peer.base_url,
        None,
        WORKSPACE_IDENTITY,
        "remote-room",
    );
    add_peer_mapping_url(
        &fixture,
        "z-origin",
        &peer.base_url,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
        "remote-room",
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response_bytes(response).await.as_ref(), PNG_1X1);
    assert_eq!(peer.object_gets(), 1);
    assert_eq!(peer.object_heads(), 0);
}

#[tokio::test(start_paused = true)]
#[serial(home_env)]
#[cfg(feature = "test-support")]
async fn fast_positive_fallback_starts_get_before_slow_head_batch_drains() {
    let fast = MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let stalled = MockPeer::spawn(
        "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa",
        PeerBehavior::StalledHead,
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "a-fast",
        &fast,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    for index in 0..12 {
        add_peer_mapping_url(
            &fixture,
            &format!("b-stalled-{index:02}"),
            &stalled.base_url,
            Some("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
            WORKSPACE_IDENTITY,
            &format!("stalled-room-{index:02}"),
        );
    }
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while (fixture.service.fallback_probe_successes() == 0 || stalled.inflight_heads() < 7)
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert_eq!(fast.object_heads(), 1);
    assert_eq!(fixture.service.fallback_probe_successes(), 1);
    assert_eq!(stalled.inflight_heads(), 7);
    assert!(!request.is_finished());
    let started = tokio::time::Instant::now();
    tokio::time::advance(Duration::from_secs(10)).await;
    stalled.release_heads();
    stalled.wait_for_heads_to_finish().await;
    for _ in 0..1_000 {
        if fixture.service.fallback_probe_windows_completed() == 1 {
            break;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
    }
    assert_eq!(fixture.service.fallback_probe_windows_completed(), 1);
    for _ in 0..1_000 {
        if fast.object_gets() == 1 {
            break;
        }
        tokio::time::advance(Duration::from_millis(1)).await;
    }
    assert_eq!(fast.object_gets(), 1);
    for _ in 0..100_000 {
        if request.is_finished() {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert!(request.is_finished());
    assert!(started.elapsed() >= Duration::from_secs(10));
    assert!(started.elapsed() < Duration::from_secs(11));
    let response = request.await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::OK,
        "fast_heads={} fast_gets={} stalled_heads={} elapsed={:?}",
        fast.object_heads(),
        fast.object_gets(),
        stalled.object_heads(),
        started.elapsed()
    );
    assert_eq!(response_bytes(response).await.as_ref(), PNG_1X1);
    assert_eq!(fast.object_gets(), 1);
    assert_eq!(stalled.object_gets(), 0);
    assert_eq!(stalled.object_heads(), 7);
    assert!(started.elapsed() < Duration::from_secs(120));
    assert!(stalled.max_inflight_heads() <= 8);
}

#[tokio::test]
async fn concurrent_gets_issue_one_peer_object_get() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_millis(100),
        },
    )
    .await;
    let first_fixture = fixture();
    add_peer(
        &first_fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let hash = sha256(PNG_1X1);
    let responses = futures::future::join_all((0..8).map(|_| {
        let app = first_fixture.app.clone();
        let hash = hash.clone();
        async move {
            app.oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
                .await
                .unwrap()
        }
    }))
    .await;

    assert!(responses
        .iter()
        .all(|response| response.status() == StatusCode::OK));
    assert_eq!(peer.object_gets(), 1);
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn cross_process_resolvers_share_filesystem_singleflight() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_millis(100),
        },
    )
    .await;
    let workspace = tempfile::tempdir().unwrap();
    let store = gitim_runtime::assets::AssetStore::open(
        workspace.path(),
        format!("github:{WORKSPACE_IDENTITY}"),
        limits(),
    )
    .unwrap();
    let hash = sha256(PNG_1X1);
    let start = workspace.path().join("resolver-start");
    let ready_a = workspace.path().join("resolver-ready-a");
    let ready_b = workspace.path().join("resolver-ready-b");
    let mut child_a =
        spawn_resolver_child(workspace.path(), &peer.base_url, &hash, &start, &ready_a);
    let mut child_b =
        spawn_resolver_child(workspace.path(), &peer.base_url, &hash, &start, &ready_b);
    wait_for_child_marker(&mut child_a, &ready_a);
    wait_for_child_marker(&mut child_b, &ready_b);
    std::fs::write(&start, b"start").unwrap();

    let output_a = tokio::task::spawn_blocking(move || child_a.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    let output_b = tokio::task::spawn_blocking(move || child_b.wait_with_output())
        .await
        .unwrap()
        .unwrap();

    assert!(
        output_a.status.success(),
        "{}",
        String::from_utf8_lossy(&output_a.stderr)
    );
    assert!(
        output_b.status.success(),
        "{}",
        String::from_utf8_lossy(&output_b.stderr)
    );
    assert_eq!(peer.object_gets(), 1);
    assert_eq!(object_files(workspace.path()).len(), 1);
    assert!(temp_files(workspace.path()).is_empty());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert_eq!(store.inspect(&hash).unwrap().size, PNG_1X1.len() as u64);
    let lock = tokio::time::timeout(
        Duration::from_secs(2),
        gitim_runtime::assets::HashLock::acquire(&store, &hash),
    )
    .await
    .unwrap()
    .unwrap();
    drop(lock);
}

#[test]
#[cfg(feature = "test-support")]
#[ignore = "child-process helper"]
fn fleet_resolver_child() {
    if std::env::var_os("GITIM_FLEET_RESOLVER_CHILD").is_none() {
        return;
    }
    let workspace = PathBuf::from(std::env::var_os("GITIM_FLEET_RESOLVER_WORKSPACE").unwrap());
    let base_url = std::env::var("GITIM_FLEET_RESOLVER_BASE_URL").unwrap();
    let hash = std::env::var("GITIM_FLEET_RESOLVER_HASH").unwrap();
    let start = PathBuf::from(std::env::var_os("GITIM_FLEET_RESOLVER_START").unwrap());
    let ready = PathBuf::from(std::env::var_os("GITIM_FLEET_RESOLVER_READY").unwrap());
    let store = gitim_runtime::assets::AssetStore::open(
        &workspace,
        format!("github:{WORKSPACE_IDENTITY}"),
        limits(),
    )
    .unwrap();
    std::fs::write(ready, b"ready").unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !start.exists() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let (_router, state) = create_router();
        add_peer_mapping_url_to_state(
            &state,
            "origin",
            &base_url,
            Some(ORIGIN_RUNTIME_ID),
            WORKSPACE_IDENTITY,
            "remote-room",
        );
        let service = Arc::new(AssetService::new(limits()));
        let replica = resolve_fleet_asset_for_test(
            &state,
            &service,
            &store,
            "room",
            WORKSPACE_IDENTITY,
            ORIGIN_RUNTIME_ID,
            &hash,
        )
        .await
        .unwrap();
        assert_eq!(replica.metadata.sha256, hash);
        assert_eq!(replica.metadata.size, PNG_1X1.len() as u64);
        assert_eq!(store.reserved_bytes().unwrap(), 0);
        assert_eq!(service.available_peer_permits(), limits().peer_slots);
    });
}

#[tokio::test]
async fn exact_origin_wrong_hash_falls_back_and_all_wrong_hashes_win_precedence() {
    let wrong = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::Wrong(b"wrong-origin".to_vec()),
    )
    .await;
    let good = MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let first_fixture = fixture();
    add_peer(
        &first_fixture,
        "origin",
        &wrong,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &first_fixture,
        "replica",
        &good,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let response = first_fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(wrong.object_gets(), 1);
    assert_eq!(good.object_gets(), 1);

    let wrong_a =
        MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Wrong(b"wrong-a".to_vec())).await;
    let wrong_b = MockPeer::spawn(
        FALLBACK_RUNTIME_ID,
        PeerBehavior::Wrong(b"wrong-b".to_vec()),
    )
    .await;
    let malformed = MockPeer::spawn(
        "99999999-9999-4999-8999-999999999999",
        PeerBehavior::Malformed,
    )
    .await;
    let second = fixture();
    add_peer(
        &second,
        "origin",
        &wrong_a,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &second,
        "replica",
        &wrong_b,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &second,
        "malformed",
        &malformed,
        Some("99999999-9999-4999-8999-999999999999"),
        WORKSPACE_IDENTITY,
    );
    let response = second
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_hash_mismatch"
    );
}

#[tokio::test]
async fn local_verified_hash_wins_without_contacting_stale_origin() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Missing).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let token = fixture
        .state
        .lock()
        .unwrap()
        .workspaces
        .get("room")
        .unwrap()
        .asset_token
        .clone();
    let store = fixture
        .service
        .open_registered_store(
            fixture.workspace.path(),
            &format!("github:{WORKSPACE_IDENTITY}"),
            &token,
        )
        .unwrap();
    let metadata = store.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            "ffffffff-ffff-4fff-8fff-ffffffffffff",
            &metadata.sha256,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(peer.object_gets(), 0);
    assert_eq!(peer.object_heads(), 0);
}

#[tokio::test(start_paused = true)]
#[cfg(feature = "test-support")]
async fn resolve_route_bounds_initial_local_verification() {
    let fixture = fixture();
    let store = workspace_store(&fixture);
    let metadata = store.put_bytes(PNG_1X1, AssetSource::LocalUpload).unwrap();
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_local_verification_pause_after(0, Arc::clone(&reached), Arc::clone(&resume));
    let app = fixture.app.clone();
    let hash = metadata.sha256.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
            .await
            .unwrap()
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();

    let responsive = tokio::spawn(async {
        tokio::task::yield_now().await;
        true
    });
    assert!(responsive.await.unwrap());
    tokio::time::advance(Duration::from_secs(121)).await;
    tokio::task::yield_now().await;
    let finished_within_budget = request.is_finished();
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let response = request.await.unwrap();

    assert!(finished_within_budget);
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
}

#[tokio::test]
async fn head_remote_availability_never_persists_or_sends_browser_headers() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::HEAD,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream",
        "remote HEAD validates but does not trust peer MIME metadata"
    );
    assert_eq!(peer.object_gets(), 0);
    assert_eq!(peer.object_heads(), 1);
    assert!(object_files(fixture.workspace.path()).is_empty());
    assert!(!peer.browser_headers_seen());
}

#[tokio::test]
async fn remote_alias_named_local_remains_remote_head_availability() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "local",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::HEAD,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(peer.object_heads(), 1);
    assert_eq!(peer.object_gets(), 0);
    assert!(object_files(fixture.workspace.path()).is_empty());
}

#[tokio::test]
async fn remote_head_honors_exact_strong_validator_and_immutable_headers() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let hash = sha256(PNG_1X1);
    let etag = format!("\"sha256-{hash}\"");
    let uri = format!("/workspaces/room/assets/resolve/{ORIGIN_RUNTIME_ID}/{hash}?name=pixel.png");

    let ok = fixture
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
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::ETAG], etag);
    assert_eq!(
        ok.headers()[header::CACHE_CONTROL],
        "private, immutable, max-age=31536000"
    );
    assert_eq!(ok.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(ok.headers()["x-content-type-options"], "nosniff");
    assert!(response_bytes(ok).await.is_empty());

    let not_modified = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri(&uri)
                .header(header::IF_NONE_MATCH, &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.headers()[header::ETAG], etag);
    assert_eq!(
        not_modified.headers()[header::CACHE_CONTROL],
        "private, immutable, max-age=31536000"
    );
    assert_eq!(not_modified.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(not_modified.headers()["x-content-type-options"], "nosniff");
    assert!(response_bytes(not_modified).await.is_empty());

    let mut conditional_range = Request::builder()
        .method(Method::HEAD)
        .uri(&uri)
        .header(header::IF_NONE_MATCH, &etag)
        .body(Body::empty())
        .unwrap();
    conditional_range
        .headers_mut()
        .append(header::RANGE, "bytes=0-1".parse().unwrap());
    conditional_range
        .headers_mut()
        .append(header::RANGE, "bytes=2-3".parse().unwrap());
    let conditional_range = fixture
        .app
        .clone()
        .oneshot(conditional_range)
        .await
        .unwrap();
    assert_eq!(conditional_range.status(), StatusCode::NOT_MODIFIED);
    assert!(response_bytes(conditional_range).await.is_empty());

    let weak = fixture
        .app
        .oneshot(
            Request::builder()
                .method(Method::HEAD)
                .uri(&uri)
                .header(header::IF_NONE_MATCH, format!("W/{etag}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(weak.status(), StatusCode::OK);
    assert_eq!(peer.object_heads(), 4);
    assert_eq!(peer.object_gets(), 0);
    assert!(object_files(fixture.workspace.path()).is_empty());
}

#[tokio::test]
async fn workspace_identity_filters_candidates_before_network_access() {
    let wrong_identity =
        MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "wrong-identity",
        &wrong_identity,
        Some(ORIGIN_RUNTIME_ID),
        "github.com/other/room",
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_origin_unavailable"
    );
    assert_eq!(wrong_identity.object_gets(), 0);
    assert_eq!(wrong_identity.object_heads(), 0);
}

#[tokio::test]
async fn no_eligible_peer_mapping_is_unavailable_for_get_and_head() {
    let fixture = fixture();
    let hash = sha256(PNG_1X1);

    let get = fixture
        .app
        .clone()
        .oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        response_json(get).await["error_code"],
        "asset_origin_unavailable"
    );

    let head = fixture
        .app
        .oneshot(resolve_request(Method::HEAD, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        2
    );
}

#[tokio::test]
async fn invalid_and_unreachable_peer_urls_are_unavailable() {
    let (unreachable, accepts, reset_peer) = spawn_accept_close_peer().await;
    for base_url in ["://invalid".to_string(), unreachable] {
        let fixture = fixture();
        add_peer_url(
            &fixture,
            "origin",
            &base_url,
            Some(ORIGIN_RUNTIME_ID),
            WORKSPACE_IDENTITY,
        );

        let response = fixture
            .app
            .oneshot(resolve_request(
                Method::GET,
                ORIGIN_RUNTIME_ID,
                &sha256(PNG_1X1),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            response_json(response).await["error_code"],
            "asset_origin_unavailable"
        );
        assert_eq!(
            fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
            1
        );
    }
    assert!(accepts.load(Ordering::Acquire) > 0);
    reset_peer.abort();
}

#[tokio::test]
async fn malformed_and_oversized_peer_responses_are_bad_gateway() {
    for behavior in [
        PeerBehavior::Malformed,
        PeerBehavior::Oversized(51 * 1024 * 1024),
    ] {
        let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, behavior).await;
        let fixture = fixture();
        add_peer(
            &fixture,
            "origin",
            &peer,
            Some(ORIGIN_RUNTIME_ID),
            WORKSPACE_IDENTITY,
        );
        let response = fixture
            .app
            .oneshot(resolve_request(
                Method::GET,
                ORIGIN_RUNTIME_ID,
                &sha256(PNG_1X1),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            response_json(response).await["error_code"],
            "asset_peer_invalid"
        );
        assert_eq!(
            fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
            1
        );
    }
}

#[tokio::test]
async fn peer_response_requires_immutable_object_metadata() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::IncompleteMetadata(PNG_1X1.to_vec()),
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_peer_invalid"
    );
    assert!(object_files(fixture.workspace.path()).is_empty());
}

#[tokio::test]
async fn truncated_content_length_is_a_malformed_peer_response() {
    let hash = sha256(PNG_1X1);
    let (base_url, task) = spawn_raw_truncated_peer(hash.clone()).await;
    let fixture = fixture();
    add_peer_url(
        &fixture,
        "origin",
        &base_url,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_peer_invalid"
    );
    assert!(temp_files(fixture.workspace.path()).is_empty());
    task.abort();
}

#[tokio::test]
async fn quota_failure_retains_local_status_and_does_not_persist() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture_with_limits(AssetLimits {
        workspace_quota_bytes: 1,
        min_free_bytes: 1,
        ..limits()
    });
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::INSUFFICIENT_STORAGE);
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_quota_exceeded"
    );
    assert!(object_files(fixture.workspace.path()).is_empty());
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        0,
        "local quota rejection is not a Fleet transport failure"
    );
}

#[tokio::test]
async fn peer_transfer_slots_cap_concurrent_object_gets_at_four() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_millis(150),
        },
    )
    .await;
    let fixture = fixture_with_limits(AssetLimits {
        peer_slots: 4,
        ..limits()
    });
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let responses = futures::future::join_all((0..8).map(|index| {
        let app = fixture.app.clone();
        let hash = sha256(format!("object-{index}").as_bytes());
        async move {
            app.oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
                .await
                .unwrap()
        }
    }))
    .await;
    assert!(responses
        .iter()
        .all(|response| { matches!(response.status(), StatusCode::BAD_GATEWAY | StatusCode::OK) }));
    assert!(
        peer.max_inflight_gets() <= 4,
        "global peer transfer cap exceeded"
    );
}

#[tokio::test]
async fn unreachable_exact_origin_falls_back_to_workspace_replica() {
    let origin = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Missing).await;
    let fallback =
        MockPeer::spawn(FALLBACK_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &origin,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "fallback",
        &fallback,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    origin.shutdown();

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(fallback.object_heads(), 1);
    assert_eq!(fallback.object_gets(), 1);
}

#[tokio::test]
async fn head_distinguishes_reachable_missing_from_unavailable() {
    let missing = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Missing).await;
    let missing_fixture = fixture();
    add_peer(
        &missing_fixture,
        "missing",
        &missing,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let response = missing_fixture
        .app
        .oneshot(resolve_request(
            Method::HEAD,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);

    let unavailable = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Missing).await;
    let unavailable_fixture = fixture();
    add_peer(
        &unavailable_fixture,
        "unavailable",
        &unavailable,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    unavailable.shutdown();
    let response = unavailable_fixture
        .app
        .oneshot(resolve_request(
            Method::HEAD,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn cancelled_remote_get_releases_temp_reservation_hash_lock_and_peer_slot() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::StalledBody { declared_size: 8 },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let hash = sha256(b"expected");
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
            .await
            .unwrap()
    });
    tokio::time::timeout(Duration::from_secs(2), peer.state.get_entered.notified())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(25)).await;
    request.abort();
    assert!(request.await.is_err());
    tokio::time::sleep(Duration::from_millis(25)).await;

    let store = workspace_store(&fixture);
    assert_eq!(store.reserved_bytes().unwrap(), 0);
    assert!(temp_files(fixture.workspace.path()).is_empty());
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    let lock = gitim_runtime::assets::HashLock::acquire(&store, &sha256(b"expected"))
        .await
        .unwrap();
    drop(lock);
}

#[tokio::test]
#[serial(home_env)]
async fn legacy_resolution_succeeds_while_health_backfill_persists_identity() {
    let home = HomeGuard::install();
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    let entry = FleetNodeEntry {
        node_id: "legacy".to_string(),
        runtime_id: None,
        base_url: peer.base_url.clone(),
        node_ip: None,
        node_name: None,
        workspaces: vec!["remote-room".to_string()],
        workspace_mappings: vec![FleetWorkspaceMapping {
            remote_workspace_id: "remote-room".to_string(),
            local_workspace_id: "room".to_string(),
            workspace_identity: WORKSPACE_IDENTITY.to_string(),
        }],
        ssh_tunnel: None,
    };
    gitim_runtime::user_config::write_to(
        &gitim_runtime::user_config::UserConfig {
            fleet_nodes: vec![entry.clone()],
            ..Default::default()
        },
        &home.path().join(".gitim/runtime.json"),
    )
    .unwrap();
    fleet::activate_node(fixture.state.clone(), entry);

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(peer.object_gets(), 1);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.state.lock().unwrap().fleet_nodes["legacy"]
        .entry
        .runtime_id
        .is_none()
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        fixture.state.lock().unwrap().fleet_nodes["legacy"]
            .entry
            .runtime_id
            .as_deref(),
        Some(ORIGIN_RUNTIME_ID)
    );
    let persisted =
        gitim_runtime::user_config::read_from(Some(&home.path().join(".gitim/runtime.json")));
    assert_eq!(
        persisted.fleet_nodes[0].runtime_id.as_deref(),
        Some(ORIGIN_RUNTIME_ID)
    );
}

#[tokio::test]
async fn duplicate_runtime_candidates_issue_one_exact_object_get() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "alpha",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "beta",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(peer.object_gets(), 1);
}

#[tokio::test]
async fn runtime_health_reports_real_fleet_and_integrity_counters() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Wrong(b"wrong".to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let response = fixture
        .app
        .clone()
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);

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
    let health = response_json(health).await;
    assert_eq!(health["asset_hash_mismatches"], 1);
    assert_eq!(health["asset_fleet_fetch_failures"], 1);
}

#[tokio::test(start_paused = true)]
async fn idle_peer_is_cancelled_before_candidate_deadline_and_cleans_staging() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::StalledBody { declared_size: 8 },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(b"expected"),
        ))
        .await
        .unwrap()
    });
    let store = workspace_store(&fixture);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while store.reserved_bytes().unwrap() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert_eq!(store.reserved_bytes().unwrap(), 1);
    let started = tokio::time::Instant::now();
    let response = request.await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() >= Duration::from_secs(15),
        "elapsed {:?}",
        started.elapsed()
    );
    assert!(started.elapsed() < Duration::from_secs(90));
    assert!(temp_files(fixture.workspace.path()).is_empty());
    assert_eq!(store.reserved_bytes().unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn active_chunks_are_still_bounded_before_the_whole_resolution_deadline() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::SlowChunks {
            declared_size: 100,
            interval: Duration::from_secs(14),
        },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let started = tokio::time::Instant::now();

    let response = fixture
        .app
        .clone()
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(b"expected"),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() < Duration::from_secs(120),
        "elapsed {:?}",
        started.elapsed()
    );
    assert!(temp_files(fixture.workspace.path()).is_empty());
    assert_eq!(workspace_store(&fixture).reserved_bytes().unwrap(), 0);
}

#[tokio::test(start_paused = true)]
async fn whole_timeout_preserves_origin_hash_mismatch_precedence() {
    let origin = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::Wrong(b"wrong origin bytes".to_vec()),
    )
    .await;
    let fallback = MockPeer::spawn(
        FALLBACK_RUNTIME_ID,
        PeerBehavior::SlowChunks {
            declared_size: 100,
            interval: Duration::from_secs(14),
        },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &origin,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "fallback-a",
        &fallback,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "fallback-b",
        &fallback,
        Some("d1ed66e4-c1e2-4c0f-b4a9-3091d6279796"),
        WORKSPACE_IDENTITY,
    );
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(b"expected bytes"),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.hash_mismatches.load(Ordering::Acquire) == 0
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert_eq!(origin.object_gets(), 1);
    assert_eq!(fixture.service.hash_mismatches.load(Ordering::Acquire), 1);
    let response = request.await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "origin_gets={} fallback_gets={} hash_mismatches={}",
        origin.object_gets(),
        fallback.object_gets(),
        fixture.service.hash_mismatches.load(Ordering::Acquire)
    );
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_hash_mismatch"
    );
    assert_eq!(fixture.service.hash_mismatches.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
    assert!(temp_files(fixture.workspace.path()).is_empty());
    assert_eq!(workspace_store(&fixture).reserved_bytes().unwrap(), 0);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
}

#[tokio::test(start_paused = true)]
async fn whole_timeout_preserves_origin_peer_invalid_precedence() {
    let origin = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::IncompleteMetadata(PNG_1X1.to_vec()),
    )
    .await;
    let fallback = MockPeer::spawn(
        FALLBACK_RUNTIME_ID,
        PeerBehavior::SlowChunks {
            declared_size: 100,
            interval: Duration::from_secs(14),
        },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &origin,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "fallback-a",
        &fallback,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "fallback-b",
        &fallback,
        Some("d1ed66e4-c1e2-4c0f-b4a9-3091d6279796"),
        WORKSPACE_IDENTITY,
    );

    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(b"expected bytes"),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fallback.object_heads() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert_eq!(origin.object_gets(), 1);
    assert!(fallback.object_heads() > 0);
    let response = request.await.unwrap();

    assert_eq!(
        response.status(),
        StatusCode::BAD_GATEWAY,
        "origin_gets={} fallback_gets={} hash_mismatches={}",
        origin.object_gets(),
        fallback.object_gets(),
        fixture.service.hash_mismatches.load(Ordering::Acquire)
    );
    assert_eq!(
        response_json(response).await["error_code"],
        "asset_peer_invalid"
    );
    assert_eq!(fixture.service.hash_mismatches.load(Ordering::Acquire), 0);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
    assert!(temp_files(fixture.workspace.path()).is_empty());
    assert_eq!(workspace_store(&fixture).reserved_bytes().unwrap(), 0);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
}

#[tokio::test(start_paused = true)]
async fn candidate_timeout_includes_waiting_for_peer_capacity() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let mut permits = Vec::new();
    for _ in 0..limits().peer_slots {
        permits.push(fixture.service.acquire_peer().await.unwrap());
    }
    let started = tokio::time::Instant::now();

    let response = fixture
        .app
        .oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() >= Duration::from_secs(90));
    assert!(started.elapsed() < Duration::from_secs(91));
    assert_eq!(peer.object_gets(), 0);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
    drop(permits);
    tokio::task::yield_now().await;
    assert_eq!(peer.object_gets(), 0);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
}

#[tokio::test(start_paused = true)]
#[cfg(feature = "test-support")]
async fn timed_out_persistence_keeps_peer_permit_and_hash_lock_until_settled() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let store = workspace_store(&fixture);
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let hash = sha256(PNG_1X1);
    let app = fixture.app.clone();
    let request_hash = hash.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &request_hash,
        ))
        .await
        .unwrap()
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();

    tokio::time::advance(Duration::from_secs(91)).await;
    let response = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("candidate timeout must finish the request")
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let permits_while_settling = fixture.service.available_peer_permits();
    let contender_store = store.clone();
    let contender_hash = hash.clone();
    let contender_finished = Arc::new(AtomicBool::new(false));
    let contender_signal = Arc::clone(&contender_finished);
    let contender = std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        let result = runtime.block_on(gitim_runtime::assets::HashLock::acquire(
            &contender_store,
            &contender_hash,
        ));
        contender_signal.store(true, Ordering::Release);
        result
    });
    std::thread::sleep(Duration::from_millis(25));
    let contender_waited = !contender_finished.load(Ordering::Acquire);

    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let lock = tokio::task::spawn_blocking(move || contender.join().unwrap())
        .await
        .unwrap()
        .unwrap();
    drop(lock);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.available_peer_permits() != limits().peer_slots
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }

    assert_eq!(permits_while_settling, limits().peer_slots - 1);
    assert!(contender_waited);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    assert!(store.inspect(&hash).is_ok());
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
    assert_eq!(fixture.service.store_failures.load(Ordering::Acquire), 0);
}

#[tokio::test]
#[cfg(feature = "test-support")]
async fn cancelled_persistence_keeps_owned_settlement_alive() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let store = workspace_store(&fixture);
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let hash = sha256(PNG_1X1);
    let app = fixture.app.clone();
    let request_hash = hash.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &request_hash,
        ))
        .await
        .unwrap()
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();
    request.abort();
    assert!(request.await.is_err());
    tokio::task::yield_now().await;
    let permits_while_settling = fixture.service.available_peer_permits();

    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.available_peer_permits() != limits().peer_slots
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }

    assert_eq!(permits_while_settling, limits().peer_slots - 1);
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    assert!(store.inspect(&hash).is_ok());
}

#[tokio::test(start_paused = true)]
#[cfg(feature = "test-support")]
async fn detached_settlement_records_eventual_store_failure_once() {
    let peer = MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let store = workspace_store(&fixture);
    store.inject_after_publish_failure_once();
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let events = Arc::new(Mutex::new(Vec::<AssetEvent>::new()));
    let observed = Arc::clone(&events);
    fixture
        .service
        .set_event_observer(Arc::new(move |event| observed.lock().unwrap().push(event)));
    let hash = sha256(PNG_1X1);
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(Method::GET, ORIGIN_RUNTIME_ID, &hash))
            .await
            .unwrap()
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(91)).await;
    let response = tokio::time::timeout(Duration::from_secs(1), request)
        .await
        .expect("candidate timeout must finish the request")
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.store_failures.load(Ordering::Acquire) == 0
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }

    assert_eq!(fixture.service.store_failures.load(Ordering::Acquire), 1);
    assert_eq!(
        fixture.service.fleet_fetch_failures.load(Ordering::Acquire),
        1
    );
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
    assert_eq!(
        events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event.event == "asset_store_failure")
            .count(),
        1
    );
}

#[tokio::test(start_paused = true)]
#[cfg(feature = "test-support")]
async fn detached_settlement_counts_toward_the_four_peer_slot_cap() {
    let settling_peer =
        MockPeer::spawn(ORIGIN_RUNTIME_ID, PeerBehavior::Object(PNG_1X1.to_vec())).await;
    let waiting_bytes = b"second object";
    let waiting_peer = MockPeer::spawn(
        FALLBACK_RUNTIME_ID,
        PeerBehavior::Object(waiting_bytes.to_vec()),
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "settling",
        &settling_peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    add_peer(
        &fixture,
        "waiting",
        &waiting_peer,
        Some(FALLBACK_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let store = workspace_store(&fixture);
    let reached = Arc::new(Barrier::new(2));
    let resume = Arc::new(Barrier::new(2));
    store.inject_before_publish_pause(Arc::clone(&reached), Arc::clone(&resume));
    let app = fixture.app.clone();
    let first = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    tokio::task::spawn_blocking(move || reached.wait())
        .await
        .unwrap();
    tokio::time::advance(Duration::from_secs(91)).await;
    let response = tokio::time::timeout(Duration::from_secs(1), first)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);

    let mut manual_permits = Vec::new();
    for _ in 0..3 {
        manual_permits.push(fixture.service.acquire_peer().await.unwrap());
    }
    assert_eq!(fixture.service.available_peer_permits(), 0);
    let waiting_workspace = tempfile::tempdir().unwrap();
    let waiting_store = gitim_runtime::assets::AssetStore::open(
        waiting_workspace.path(),
        format!("github:{WORKSPACE_IDENTITY}"),
        limits(),
    )
    .unwrap();
    let waiting_state = fixture.state.clone();
    let waiting_service = Arc::clone(&fixture.service);
    let waiting_hash = sha256(waiting_bytes);
    let waiter = tokio::spawn(async move {
        resolve_fleet_asset_for_test(
            &waiting_state,
            &waiting_service,
            &waiting_store,
            "room",
            WORKSPACE_IDENTITY,
            FALLBACK_RUNTIME_ID,
            &waiting_hash,
        )
        .await
    });
    for _ in 0..100 {
        tokio::task::yield_now().await;
    }
    assert_eq!(waiting_peer.object_gets(), 0);

    drop(manual_permits.pop());
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !waiter.is_finished() && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(waiter.await.unwrap().is_ok());
    drop(manual_permits);
    tokio::task::spawn_blocking(move || resume.wait())
        .await
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while fixture.service.available_peer_permits() != limits().peer_slots
        && std::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert_eq!(
        fixture.service.available_peer_permits(),
        limits().peer_slots
    );
}

#[tokio::test(start_paused = true)]
async fn response_header_timeout_is_ten_seconds() {
    let peer = MockPeer::spawn(
        ORIGIN_RUNTIME_ID,
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_secs(30),
        },
    )
    .await;
    let fixture = fixture();
    add_peer(
        &fixture,
        "origin",
        &peer,
        Some(ORIGIN_RUNTIME_ID),
        WORKSPACE_IDENTITY,
    );
    let app = fixture.app.clone();
    let request = tokio::spawn(async move {
        app.oneshot(resolve_request(
            Method::GET,
            ORIGIN_RUNTIME_ID,
            &sha256(PNG_1X1),
        ))
        .await
        .unwrap()
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while peer.object_gets() == 0 && std::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert_eq!(peer.object_gets(), 1);
    let started = tokio::time::Instant::now();
    let response = request.await.unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        started.elapsed() >= Duration::from_secs(10),
        "elapsed {:?}",
        started.elapsed()
    );
    assert!(
        started.elapsed() < Duration::from_secs(11),
        "elapsed {:?}",
        started.elapsed()
    );
}

#[tokio::test]
async fn fallback_head_fanout_caps_at_eight_and_full_gets_remain_sequential() {
    let peer = MockPeer::spawn(
        FALLBACK_RUNTIME_ID,
        PeerBehavior::HeaderDelay {
            bytes: PNG_1X1.to_vec(),
            delay: Duration::from_millis(75),
        },
    )
    .await;
    let fixture = fixture();
    for index in 0..12 {
        let runtime_id = format!("00000000-0000-4000-8000-{index:012}");
        add_peer_mapping_url(
            &fixture,
            &format!("peer-{index:02}"),
            &peer.base_url,
            Some(&runtime_id),
            WORKSPACE_IDENTITY,
            &format!("remote-room-{index:02}"),
        );
    }
    let origin = "ffffffff-ffff-4fff-8fff-ffffffffffff";
    let head = fixture
        .app
        .clone()
        .oneshot(resolve_request(Method::HEAD, origin, &sha256(PNG_1X1)))
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(peer.max_inflight_heads(), 8);

    let get = fixture
        .app
        .oneshot(resolve_request(Method::GET, origin, &sha256(b"different")))
        .await
        .unwrap();
    assert_eq!(get.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(
        response_json(get).await["error_code"],
        "asset_hash_mismatch"
    );
    assert_eq!(peer.max_inflight_gets(), 1);
}
