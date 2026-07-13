# Mobile Gateway Discovery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let GitHub-backed Browser/WASM workspaces discover trusted Tailscale Gateway Runtimes from Git and use the existing GitHub token for authenticated attachment upload and resolution.

**Architecture:** Runtime publishes a validated, low-churn Gateway descriptor into each enabled Git repository. Browser/WASM reads those descriptors, verifies a live Runtime identity, exchanges its GitHub token for a ten-minute workspace-scoped session, and sends authenticated asset requests to a dedicated mobile listener that reuses the existing asset store and Fleet resolver.

**Tech Stack:** Rust stable, Axum, Tokio, reqwest, GitIM daemon IPC, `gitim-core`, `gitim-wasm`, React 19, TypeScript, Web Worker RPC, isomorphic-git, Vitest, Playwright, Tailscale HTTPS ingress.

---

## File structure

- Create: `crates/gitim-core/src/types/gateway.rs`
  - Owns the versioned Gateway descriptor, capability constants, validation, and canonical endpoint rules.
- Create: `crates/gitim-core/tests/gateway_meta_test.rs`
  - Locks valid YAML, rejection boundaries, and stable serialization.
- Modify: `crates/gitim-core/src/types/mod.rs`
  - Exports Gateway protocol types.
- Modify: `crates/gitim-core/Cargo.toml`
  - Adds direct WASM-compatible `url` and `uuid` dependencies used for endpoint and Runtime identity validation.
- Create: `crates/gitim-daemon/src/handlers/gateway.rs`
  - Implements daemon-owned publish, remove, and list operations under `gateways/`.
- Modify: `crates/gitim-daemon/src/api.rs`
  - Adds Gateway IPC requests.
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
  - Dispatches Gateway IPC and classifies mutations as writes.
- Modify: `crates/gitim-client/src/client.rs`
  - Adds typed Gateway publish/remove/list calls for Runtime.
- Modify: `crates/gitim-runtime/src/user_config.rs`
  - Persists the enabled mobile endpoint, priority, and workspace allowlist.
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`
  - Adds `gateway enable|disable|status` and starts the dedicated mobile listener when enabled.
- Modify: `crates/gitim-runtime/src/github.rs`
  - Returns GitHub repository pull/push permissions during token verification.
- Modify: `crates/gitim-runtime/Cargo.toml`
  - Adds `secrecy` so presented GitHub credentials cannot leak through derived debug output.
- Create: `crates/gitim-runtime/src/mobile_gateway/mod.rs`
  - Owns mobile listener assembly and shared state.
- Create: `crates/gitim-runtime/src/mobile_gateway/auth.rs`
  - Owns token exchange, opaque sessions, expiry, and permission checks.
- Create: `crates/gitim-runtime/src/mobile_gateway/http.rs`
  - Exposes the mobile route allowlist and delegates authorized requests to assets.
- Modify: `crates/gitim-runtime/src/assets/http.rs`
  - Extracts workspace-authorized upload/resolve functions shared by local and mobile routers.
- Modify: `crates/gitim-runtime/src/http.rs`
  - Supplies workspace identity lookup and the injectable GitHub verifier to mobile auth.
- Modify: `crates/gitim-runtime/src/lib.rs`
  - Exports the mobile Gateway module.
- Create: `crates/gitim-runtime/tests/mobile_gateway_auth.rs`
  - Covers GitHub permission mapping, session expiry, identity binding, and token redaction.
- Create: `crates/gitim-runtime/tests/mobile_gateway_http.rs`
  - Covers route allowlisting, CORS, authenticated upload/resolve, Fleet fallback, and workspace isolation.
- Modify: `crates/gitim-wasm/src/lib.rs`
  - Exposes authoritative Gateway YAML parsing to Browser/WASM.
- Rebuild: `crates/gitim-wasm/pkg/`
  - Ships the new parser binding.
- Create: `products/gitim/frontend/src/daemon-web/gateways.ts`
  - Reads and validates Gateway descriptors from the browser Git filesystem.
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
  - Adds `listGateways` to the Worker-owned repository surface.
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
  - Adds Gateway list RPC typing and dispatch.
- Modify: `products/gitim/frontend/src/lib/backend.ts`
  - Exposes Gateway list RPC through `LocalBackend`.
- Create: `products/gitim/frontend/src/lib/gateway-client.ts`
  - Owns candidate selection, live identity verification, token exchange, session refresh, and failover.
- Create: `products/gitim/frontend/src/lib/gateway-client.test.ts`
  - Covers deterministic selection and failure behavior.
- Modify: `products/gitim/frontend/src/lib/client.ts`
  - Routes Browser/WASM asset upload and resolve through the active Gateway.
- Modify: `products/gitim/frontend/src/components/chat/asset-fragment.tsx`
  - Loads authenticated Gateway images into revocable object URLs and retains metadata fallback.
- Modify: `products/gitim/frontend/src/components/chat/message-body.test.tsx`
  - Covers Gateway-ready, auth-expired, failover, and metadata-only rendering.
- Modify: `products/gitim/frontend/src/lib/client.assets.test.ts`
  - Covers Browser/WASM upload through Gateway sessions.
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`
  - Covers mobile discovery, upload, rendering, download, and fallback.

---

### Task 1: Define the Gateway descriptor protocol

**Files:**
- Create: `crates/gitim-core/src/types/gateway.rs`
- Create: `crates/gitim-core/tests/gateway_meta_test.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`
- Modify: `crates/gitim-core/Cargo.toml`

- [ ] **Step 1: Write the failing Gateway metadata tests**

Create `crates/gitim-core/tests/gateway_meta_test.rs`:

```rust
use gitim_core::types::{GatewayCapability, GatewayMeta};

const YAML: &str = r#"schema_version: 1
runtime_id: 3c6a295e-744a-41dc-ba60-5c21bb94e5a2
endpoint: https://macbook.example-tailnet.ts.net
capabilities:
  - assets.read.v1
  - assets.write.v1
priority: 100
published_by: lewisliu
"#;

#[test]
fn gateway_meta_round_trips_canonically() {
    let meta = GatewayMeta::from_yaml(YAML).unwrap();
    assert_eq!(meta.schema_version, 1);
    assert_eq!(meta.priority, 100);
    assert!(meta.capabilities.contains(&GatewayCapability::AssetsReadV1));
    let reparsed = GatewayMeta::from_yaml(&meta.to_yaml().unwrap()).unwrap();
    assert_eq!(reparsed, meta);
}

#[test]
fn gateway_meta_rejects_non_origin_endpoint() {
    let yaml = YAML.replace(
        "https://macbook.example-tailnet.ts.net",
        "https://macbook.example-tailnet.ts.net/mobile?token=x",
    );
    assert!(GatewayMeta::from_yaml(&yaml).is_err());
}

#[test]
fn gateway_meta_rejects_unknown_capability() {
    let yaml = YAML.replace("assets.write.v1", "runtime.admin.v1");
    assert!(GatewayMeta::from_yaml(&yaml).is_err());
}
```

- [ ] **Step 2: Run the protocol tests and verify they fail**

Run:

```bash
cargo test -p gitim-core --test gateway_meta_test --locked
```

Expected: FAIL because `GatewayMeta` and `GatewayCapability` do not exist.

- [ ] **Step 3: Implement the authoritative Gateway type**

Add `url = "2"` and `uuid = { version = "1" }` to
`crates/gitim-core/Cargo.toml`, export `gateway` from `types/mod.rs`, and create
`types/gateway.rs` with this public surface:

```rust
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GatewayCapability {
    #[serde(rename = "assets.read.v1")]
    AssetsReadV1,
    #[serde(rename = "assets.write.v1")]
    AssetsWriteV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GatewayMeta {
    pub schema_version: u8,
    pub runtime_id: String,
    pub endpoint: String,
    pub capabilities: Vec<GatewayCapability>,
    pub priority: u16,
    pub published_by: String,
}

#[derive(Debug, Error)]
pub enum GatewayMetaError {
    #[error("unsupported gateway schema version")]
    Version,
    #[error("invalid runtime id")]
    RuntimeId,
    #[error("gateway endpoint must be an https origin")]
    Endpoint,
    #[error("gateway priority exceeds 1000")]
    Priority,
    #[error("gateway requires at least assets.read.v1")]
    Capability,
    #[error("invalid published_by handler")]
    Handler,
    #[error("invalid gateway yaml: {0}")]
    Yaml(#[from] serde_yaml::Error),
}

impl GatewayMeta {
    pub fn from_yaml(input: &str) -> Result<Self, GatewayMetaError> {
        let value: Self = serde_yaml::from_str(input)?;
        value.validate()?;
        Ok(value)
    }

    pub fn to_yaml(&self) -> Result<String, GatewayMetaError> {
        self.validate()?;
        Ok(serde_yaml::to_string(self)?)
    }

    pub fn validate(&self) -> Result<(), GatewayMetaError> {
        if self.schema_version != 1 {
            return Err(GatewayMetaError::Version);
        }
        let runtime_id = uuid::Uuid::parse_str(&self.runtime_id)
            .map_err(|_| GatewayMetaError::RuntimeId)?;
        if runtime_id.to_string() != self.runtime_id {
            return Err(GatewayMetaError::RuntimeId);
        }
        let endpoint = url::Url::parse(&self.endpoint)
            .map_err(|_| GatewayMetaError::Endpoint)?;
        let is_origin = endpoint.scheme() == "https"
            && endpoint.username().is_empty()
            && endpoint.password().is_none()
            && endpoint.path() == "/"
            && endpoint.query().is_none()
            && endpoint.fragment().is_none()
            && endpoint.host_str().is_some();
        if !is_origin {
            return Err(GatewayMetaError::Endpoint);
        }
        if self.priority > 1000 {
            return Err(GatewayMetaError::Priority);
        }
        if !self.capabilities.contains(&GatewayCapability::AssetsReadV1) {
            return Err(GatewayMetaError::Capability);
        }
        crate::types::Handler::new(&self.published_by)
            .map_err(|_| GatewayMetaError::Handler)?;
        Ok(())
    }
}
```

- [ ] **Step 4: Run core tests**

Run:

```bash
cargo test -p gitim-core --test gateway_meta_test --locked
```

Expected: 3 tests PASS.

- [ ] **Step 5: Commit the protocol slice**

```bash
git add crates/gitim-core/Cargo.toml crates/gitim-core/src/types/gateway.rs \
  crates/gitim-core/src/types/mod.rs crates/gitim-core/tests/gateway_meta_test.rs Cargo.lock
git commit -m "feat(core): define mobile gateway metadata" \
  -m "Test: cargo test -p gitim-core --test gateway_meta_test --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 2: Add daemon-owned Gateway metadata operations

**Files:**
- Create: `crates/gitim-daemon/src/handlers/gateway.rs`
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Modify: `crates/gitim-client/src/client.rs`
- Test: `crates/gitim-daemon/tests/gateway_test.rs`

- [ ] **Step 1: Write failing daemon integration tests**

Create `crates/gitim-daemon/tests/gateway_test.rs` using the existing daemon test
harness and assert these request shapes and outcomes:

```rust
#[tokio::test]
async fn current_user_can_publish_list_and_remove_gateway() {
    let env = TestEnv::new("lewisliu").await;
    let runtime_id = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
    let published = env.request(serde_json::json!({
        "method": "gateway_publish",
        "runtime_id": runtime_id,
        "endpoint": "https://macbook.example-tailnet.ts.net",
        "capabilities": ["assets.read.v1", "assets.write.v1"],
        "priority": 100
    })).await;
    assert_eq!(published["ok"], true);
    let listed = env.request(serde_json::json!({"method": "gateway_list"})).await;
    assert_eq!(listed["data"]["gateways"][0]["runtime_id"], runtime_id);
    let removed = env.request(serde_json::json!({
        "method": "gateway_remove",
        "runtime_id": runtime_id
    })).await;
    assert_eq!(removed["ok"], true);
}
```

- [ ] **Step 2: Run the daemon test and verify it fails**

Run:

```bash
cargo test -p gitim-daemon --test gateway_test --locked
```

Expected: FAIL because Gateway IPC methods are unknown.

- [ ] **Step 3: Add Gateway IPC and writer classification**

Add these variants to `crates/gitim-daemon/src/api.rs`:

```rust
#[serde(rename = "gateway_publish")]
GatewayPublish {
    runtime_id: String,
    endpoint: String,
    capabilities: Vec<gitim_core::types::GatewayCapability>,
    priority: u16,
},
#[serde(rename = "gateway_remove")]
GatewayRemove { runtime_id: String },
#[serde(rename = "gateway_list")]
GatewayList,
```

Classify `GatewayPublish` and `GatewayRemove` as writes in
`handlers/mod.rs`. Dispatch all three variants to `handlers::gateway`.

- [ ] **Step 4: Implement publish, list, and remove**

In `handlers/gateway.rs`:

```rust
pub async fn publish(
    state: SharedState,
    runtime_id: String,
    endpoint: String,
    capabilities: Vec<GatewayCapability>,
    priority: u16,
) -> Response {
    let meta = GatewayMeta {
        schema_version: 1,
        runtime_id: runtime_id.clone(),
        endpoint,
        capabilities,
        priority,
        published_by: state.current_user.clone(),
    };
    let yaml = match meta.to_yaml() {
        Ok(yaml) => yaml,
        Err(error) => return Response::error_with_code(error.to_string(), "invalid_gateway"),
    };
    let relative = format!("gateways/{runtime_id}.meta.yaml");
    let directory = state.repo_root.join("gateways");
    if let Err(error) = std::fs::create_dir_all(&directory) {
        return Response::error(format!("create gateways directory: {error}"));
    }
    let path = state.repo_root.join(&relative);
    let previous = std::fs::read(&path).ok();
    let commit_guard = state.commit_lock.lock().unwrap_or_else(|error| error.into_inner());
    if let Err(error) = std::fs::write(&path, yaml) {
        return Response::error(format!("write gateway metadata: {error}"));
    }
    let (author_name, author_email) = state.author_for(&state.current_user);
    if let Err(error) = state.git_storage.add_and_commit_as(
        &[&relative],
        &format!("gateway: publish {runtime_id}"),
        Some((&author_name, &author_email)),
    ) {
        match previous {
            Some(bytes) => { let _ = std::fs::write(&path, bytes); }
            None => { let _ = std::fs::remove_file(&path); }
        }
        return Response::error(format!("commit gateway metadata: {error}"));
    }
    drop(commit_guard);
    if let Err(error) = push_with_retry(&state, "gateway_publish").await {
        return Response::error(error);
    }
    Response::success(serde_json::json!({"runtime_id": runtime_id}))
}
```

Implement `list` by reading `gateways/*.meta.yaml`, parsing every file with
`GatewayMeta::from_yaml`, sorting by `runtime_id`, and returning
`{"gateways": [...]}`. A corrupted file returns `gateway_meta_corrupted` with
its filename. Implement `remove` under `commit_lock`, move the descriptor to
`.trash/gateways/`, commit `gateway: remove`, and restore the file if the commit
fails.

- [ ] **Step 5: Add typed client methods**

Add to `gitim-client/src/client.rs`:

```rust
pub async fn gateway_publish(&self, meta: &GatewayMeta) -> Result<ApiResponse, ClientError> {
    self.request("gateway_publish", serde_json::json!({
        "runtime_id": meta.runtime_id,
        "endpoint": meta.endpoint,
        "capabilities": meta.capabilities,
        "priority": meta.priority,
    })).await
}

pub async fn gateway_list(&self) -> Result<ApiResponse, ClientError> {
    self.request("gateway_list", serde_json::json!({})).await
}

pub async fn gateway_remove(&self, runtime_id: &str) -> Result<ApiResponse, ClientError> {
    self.request("gateway_remove", serde_json::json!({
        "runtime_id": runtime_id,
    })).await
}
```

- [ ] **Step 6: Verify daemon and client behavior**

Run:

```bash
cargo test -p gitim-daemon --test gateway_test --locked
cargo test -p gitim-client gateway --locked
```

Expected: all Gateway tests PASS.

- [ ] **Step 7: Commit the Git coordination slice**

```bash
git add crates/gitim-daemon/src/api.rs crates/gitim-daemon/src/handlers/gateway.rs \
  crates/gitim-daemon/src/handlers/mod.rs crates/gitim-daemon/tests/gateway_test.rs \
  crates/gitim-client/src/client.rs
git commit -m "feat(daemon): manage mobile gateway descriptors" \
  -m "Test: cargo test -p gitim-daemon --test gateway_test --locked" \
  -m "Test: cargo test -p gitim-client gateway --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 3: Persist Gateway configuration and publish descriptors

**Files:**
- Modify: `crates/gitim-runtime/src/user_config.rs`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`
- Test: `crates/gitim-runtime/tests/mobile_gateway_config.rs`

- [ ] **Step 1: Write failing configuration tests**

Create `crates/gitim-runtime/tests/mobile_gateway_config.rs`:

```rust
#[test]
fn mobile_gateway_config_round_trips_without_losing_fleet_nodes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("runtime.json");
    let mut config = UserConfig::default();
    config.fleet_nodes.push(sample_fleet_node());
    config.mobile_gateway = Some(MobileGatewayConfig {
        endpoint: "https://macbook.example-tailnet.ts.net".into(),
        priority: 100,
        workspace_slugs: vec!["room".into()],
        listen_port: 16869,
    });
    write_to(&config, &path).unwrap();
    let loaded = read_from(Some(&path));
    assert_eq!(loaded.mobile_gateway, config.mobile_gateway);
    assert_eq!(loaded.fleet_nodes, config.fleet_nodes);
}
```

- [ ] **Step 2: Run and verify the test fails**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_config --locked
```

Expected: FAIL because `MobileGatewayConfig` does not exist.

- [ ] **Step 3: Add Runtime configuration**

Add to `user_config.rs`:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MobileGatewayConfig {
    pub endpoint: String,
    pub priority: u16,
    pub workspace_slugs: Vec<String>,
    #[serde(default = "default_mobile_gateway_port")]
    pub listen_port: u16,
}

fn default_mobile_gateway_port() -> u16 { 16869 }

// In UserConfig:
#[serde(default, skip_serializing_if = "Option::is_none")]
pub mobile_gateway: Option<MobileGatewayConfig>,
```

Use the locked `user_config::mutate` path for enable/disable so concurrent Fleet
configuration writes are preserved.

- [ ] **Step 4: Add Runtime CLI commands**

Add Clap commands:

```rust
enum GatewayCommand {
    Enable {
        #[arg(long)] endpoint: String,
        #[arg(long = "workspace", required = true)] workspaces: Vec<String>,
        #[arg(long, default_value_t = 100)] priority: u16,
        #[arg(long, default_value_t = 16869)] listen_port: u16,
    },
    Disable,
    Status,
}
```

`enable` validates the endpoint with `GatewayMeta::validate`, verifies every
workspace slug exists and is GitHub-backed, persists config, then calls each
human daemon's `gateway_publish`. `disable` removes this Runtime's descriptor
from every configured workspace before clearing config. `status` prints the
endpoint, port, runtime ID, and configured workspaces as JSON.

- [ ] **Step 5: Verify config and CLI parsing**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_config --locked
cargo test -p gitim-runtime --bin gitim-runtime gateway --locked
```

Expected: all Gateway config and CLI tests PASS.

- [ ] **Step 6: Commit Runtime configuration**

```bash
git add crates/gitim-runtime/src/user_config.rs crates/gitim-runtime/src/bin/runtime.rs \
  crates/gitim-runtime/tests/mobile_gateway_config.rs
git commit -m "feat(runtime): configure mobile gateway publication" \
  -m "Test: cargo test -p gitim-runtime --test mobile_gateway_config --locked" \
  -m "Test: cargo test -p gitim-runtime --bin gitim-runtime gateway --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 4: Exchange GitHub access for short-lived sessions

**Files:**
- Modify: `crates/gitim-runtime/Cargo.toml`
- Modify: `crates/gitim-runtime/src/github.rs`
- Create: `crates/gitim-runtime/src/mobile_gateway/auth.rs`
- Create: `crates/gitim-runtime/src/mobile_gateway/mod.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`
- Test: `crates/gitim-runtime/tests/mobile_gateway_auth.rs`

- [ ] **Step 1: Write failing authorization tests**

Create tests that use the existing mock GitHub server:

```rust
#[tokio::test]
async fn read_only_repo_access_issues_read_session() {
    let github = MockGithub::repo_permissions(true, false);
    let authorizer = test_authorizer(github);
    let session = authorizer.exchange(SessionRequest {
        workspace_identity: "github.com/ciferateam/gitim".into(),
        github_token: "repo-read-token".into(),
    }).await.unwrap();
    assert!(session.permissions.assets_read);
    assert!(!session.permissions.assets_write);
    assert!(!session.token.contains("repo-read-token"));
}

#[tokio::test(start_paused = true)]
async fn sessions_expire_after_ten_minutes() {
    let authorizer = test_authorizer(MockGithub::repo_permissions(true, true));
    let session = authorizer.exchange(valid_request()).await.unwrap();
    tokio::time::advance(std::time::Duration::from_secs(601)).await;
    assert!(authorizer.authenticate(&session.token).is_err());
}
```

- [ ] **Step 2: Run and verify tests fail**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_auth --locked
```

Expected: FAIL because the authorizer does not exist.

- [ ] **Step 3: Return repository permissions from GitHub**

Add `secrecy = "0.10"` to `crates/gitim-runtime/Cargo.toml`, then add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RepoPermissions {
    pub pull: bool,
    pub push: bool,
}

pub async fn check_repo_permissions(
    owner: &str,
    repo: &str,
    token: &str,
    api_base: &str,
) -> Result<RepoPermissions, GithubError> {
    let url = format!("{}/repos/{owner}/{repo}", api_base.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(token)
        .header(reqwest::header::USER_AGENT, USER_AGENT)
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await?;
    match response.status().as_u16() {
        401 => return Err(GithubError::InvalidToken),
        403 => return Err(GithubError::InsufficientScope),
        404 => return Err(GithubError::RepoNotFoundOrNoAccess),
        429 => return Err(GithubError::RateLimited),
        status if (200..300).contains(&status) => {}
        status => return Err(GithubError::UnexpectedStatus(status)),
    }
    let body: RepoResponse = response.json().await?;
    Ok(RepoPermissions {
        pull: body.permissions.pull,
        push: body.permissions.push,
    })
}
```

The implementation must deserialize this exact response subset:

```rust
#[derive(Deserialize)]
struct RepoResponse {
    permissions: RepoPermissionResponse,
}

#[derive(Deserialize)]
struct RepoPermissionResponse {
    pull: bool,
    push: bool,
}
```

- [ ] **Step 4: Implement the in-memory authorizer**

Create `mobile_gateway/auth.rs`:

```rust
pub const SESSION_TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct GatewayPermissions {
    pub assets_read: bool,
    pub assets_write: bool,
}

#[derive(Debug, Clone)]
pub struct GatewaySession {
    pub token: String,
    pub workspace_slug: String,
    pub workspace_identity: String,
    pub github_login: String,
    pub permissions: GatewayPermissions,
    pub expires_at: Instant,
}

pub enum SessionCredential {
    GithubRepositoryToken {
        workspace_identity: String,
        token: secrecy::SecretString,
    },
}

#[async_trait::async_trait]
pub trait GatewaySessionAuthorizer: Send + Sync {
    async fn establish(
        &self,
        credential: SessionCredential,
    ) -> Result<GatewaySession, GatewayAuthError>;

    async fn authenticate_bearer(
        &self,
        token: &str,
    ) -> Result<GatewaySession, GatewayAuthError>;
}

pub struct GatewayAuthorizer {
    state: SharedRuntimeState,
    github: Arc<dyn MobileGithubVerifier>,
    sessions: Mutex<HashMap<String, GatewaySession>>,
}
```

`GatewayAuthorizer` implements `GatewaySessionAuthorizer`. `establish` matches the
GitHub credential, normalizes the requested identity, finds exactly one
initialized GitHub workspace with the same `GitConfig::remote_identity`, calls
`/user` and `check_repo_permissions`, rejects `pull == false`, creates a
64-hex-character token from two `Uuid::new_v4().simple()` values, stores the
session, and returns the token, permissions, and RFC 3339 expiry.
`authenticate_bearer` removes expired entries and returns a cloned session.
The HTTP router stores `Arc<dyn GatewaySessionAuthorizer>` rather than the
concrete GitHub implementation. Error display and tracing fields never include
the secret token. A future signed-device credential can add an enum variant and
authorizer without changing the asset handlers.

- [ ] **Step 5: Verify authorization behavior**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_auth --locked
```

Expected: token validation, permission mapping, workspace binding, expiry, and
redaction tests PASS.

- [ ] **Step 6: Commit Gateway authorization**

```bash
git add crates/gitim-runtime/Cargo.toml crates/gitim-runtime/src/github.rs \
  crates/gitim-runtime/src/mobile_gateway \
  crates/gitim-runtime/src/lib.rs crates/gitim-runtime/tests/mobile_gateway_auth.rs
git commit -m "feat(runtime): authorize mobile gateway sessions" \
  -m "Test: cargo test -p gitim-runtime --test mobile_gateway_auth --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 5: Expose the dedicated mobile asset surface

**Files:**
- Create: `crates/gitim-runtime/src/mobile_gateway/http.rs`
- Modify: `crates/gitim-runtime/src/mobile_gateway/mod.rs`
- Modify: `crates/gitim-runtime/src/assets/http.rs`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`
- Test: `crates/gitim-runtime/tests/mobile_gateway_http.rs`

- [ ] **Step 1: Write failing HTTP contract tests**

Add tests for the exact allowlist and workspace derivation:

```rust
#[tokio::test]
async fn authenticated_read_session_resolves_asset_without_slug_input() {
    let app = test_mobile_router().await;
    let session = create_session(&app, "repo-read-token").await;
    let response = app.oneshot(Request::get(format!(
        "/mobile/v1/assets/resolve/{ORIGIN_RUNTIME_ID}/{HASH}"
    )).header(AUTHORIZATION, format!("Bearer {session}"))).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(sha256(response.into_body()).await, HASH);
}

#[tokio::test]
async fn read_only_session_cannot_upload() {
    let app = test_mobile_router().await;
    let session = create_session(&app, "repo-read-token").await;
    let response = upload(&app, &session, b"hello").await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn administration_routes_are_absent() {
    let response = test_mobile_router().await
        .oneshot(Request::get("/workspaces"))
        .await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}
```

- [ ] **Step 2: Run and verify HTTP tests fail**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_http --locked
```

Expected: FAIL because the mobile router does not exist.

- [ ] **Step 3: Extract authorized asset service entry points**

In `assets/http.rs`, make the shared operations accept an already-authorized
workspace slug:

```rust
pub(crate) async fn resolve_authorized(
    state: SharedRuntimeState,
    workspace_slug: String,
    origin: String,
    hash: String,
    raw_query: Option<String>,
    request: Request<Body>,
) -> Response;

pub(crate) async fn upload_authorized(
    state: SharedRuntimeState,
    workspace_slug: String,
    multipart: Result<Multipart, MultipartRejection>,
) -> Response;
```

The existing local Runtime routes call the same functions after their current
browser-origin checks. The mobile router calls them after session authorization.
Keep store binding, quota, MIME inspection, SHA verification, and Fleet resolver
logic in these shared functions.

- [ ] **Step 4: Build the mobile router**

Create `mobile_gateway/http.rs`:

```rust
pub fn router(state: MobileGatewayState, allowed_origins: Vec<HeaderValue>) -> Router {
    Router::new()
        .route("/mobile/v1/hello", get(hello))
        .route("/mobile/v1/session", post(create_session))
        .route(
            "/mobile/v1/assets/resolve/{origin}/{hash}",
            get(resolve).head(resolve),
        )
        .route("/mobile/v1/assets", post(upload))
        .layer(configured_cors(allowed_origins))
        .with_state(state)
}
```

`hello` returns only runtime ID, protocol version, and capabilities. `resolve`
requires `assets_read`; `upload` requires `assets_write`. Both derive
`workspace_slug` from the authenticated session and pass it to the shared asset
entry points.

- [ ] **Step 5: Start a separate loopback listener**

In `runtime.rs`, when `mobile_gateway` is configured, bind
`127.0.0.1:<listen_port>`, start the mobile router in its own Tokio task, and log
only the loopback address plus configured public endpoint. A bind failure is
fatal when Gateway mode is enabled because the published descriptor would
otherwise advertise a dead service.

- [ ] **Step 6: Verify the mobile HTTP surface**

Run:

```bash
cargo test -p gitim-runtime --test mobile_gateway_http --locked
cargo test -p gitim-runtime --test assets_http --locked
cargo test -p gitim-runtime --test assets_fleet --locked
```

Expected: mobile auth tests and existing asset regression tests PASS.

- [ ] **Step 7: Commit the mobile Runtime surface**

```bash
git add crates/gitim-runtime/src/mobile_gateway/http.rs \
  crates/gitim-runtime/src/mobile_gateway/mod.rs crates/gitim-runtime/src/assets/http.rs \
  crates/gitim-runtime/src/bin/runtime.rs crates/gitim-runtime/tests/mobile_gateway_http.rs
git commit -m "feat(runtime): expose authenticated mobile assets" \
  -m "Test: cargo test -p gitim-runtime --test mobile_gateway_http --locked" \
  -m "Test: cargo test -p gitim-runtime --test assets_http --locked" \
  -m "Test: cargo test -p gitim-runtime --test assets_fleet --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 6: Read Gateway descriptors in Browser/WASM

**Files:**
- Modify: `crates/gitim-wasm/src/lib.rs`
- Rebuild: `crates/gitim-wasm/pkg/`
- Create: `products/gitim/frontend/src/daemon-web/gateways.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
- Modify: `products/gitim/frontend/src/lib/backend.ts`
- Test: `products/gitim/frontend/src/daemon-web/gateways.test.ts`

- [ ] **Step 1: Write failing WASM and frontend descriptor tests**

Add a wasm-bindgen test for `parseGatewayMeta` and create
`daemon-web/gateways.test.ts`:

```ts
it("lists valid descriptors in deterministic order", async () => {
  await writeGateway("b-runtime", validYaml({ priority: 10 }));
  await writeGateway("a-runtime", validYaml({ priority: 100 }));
  expect(await listGateways()).toEqual([
    expect.objectContaining({ runtime_id: "a-runtime", priority: 100 }),
    expect.objectContaining({ runtime_id: "b-runtime", priority: 10 }),
  ]);
});
```

- [ ] **Step 2: Run and verify tests fail**

Run:

```bash
cargo test -p gitim-wasm gateway --locked
cd products/gitim/frontend && npm test -- src/daemon-web/gateways.test.ts
```

Expected: FAIL because the parser and list RPC do not exist.

- [ ] **Step 3: Export authoritative YAML parsing from WASM**

Add to `gitim-wasm/src/lib.rs`:

```rust
#[wasm_bindgen(js_name = "parseGatewayMeta")]
pub fn parse_gateway_meta(yaml: &str) -> Result<JsValue, JsError> {
    let meta = gitim_core::types::GatewayMeta::from_yaml(yaml)
        .map_err(|error| JsError::new(&error.to_string()))?;
    serde_wasm_bindgen::to_value(&meta)
        .map_err(|error| JsError::new(&error.to_string()))
}
```

- [ ] **Step 4: Implement Worker-owned descriptor listing**

Create `daemon-web/gateways.ts` to list `gateways/*.meta.yaml`, reject filenames
that do not match the parsed canonical `runtime_id`, call `parseGatewayMeta`, and
sort by priority descending then runtime ID ascending. Add `listGateways` to
Worker handlers, RPC typing, and `LocalBackend`:

```ts
export interface GatewayDescriptor {
  schema_version: 1;
  runtime_id: string;
  endpoint: string;
  capabilities: ("assets.read.v1" | "assets.write.v1")[];
  priority: number;
  published_by: string;
}
```

- [ ] **Step 5: Rebuild WASM and verify**

Run:

```bash
cd products/gitim/frontend
npm run build:wasm
npm test -- src/daemon-web/gateways.test.ts src/daemon-web/wasm-semantics.test.ts
```

Expected: descriptor and WASM semantics tests PASS.

- [ ] **Step 6: Commit Browser descriptor support**

```bash
git add crates/gitim-wasm/src/lib.rs crates/gitim-wasm/pkg \
  products/gitim/frontend/src/daemon-web/gateways.ts \
  products/gitim/frontend/src/daemon-web/gateways.test.ts \
  products/gitim/frontend/src/daemon-web/handlers.ts \
  products/gitim/frontend/src/daemon-web/worker.ts \
  products/gitim/frontend/src/lib/backend.ts
git commit -m "feat(web): discover mobile gateways from git" \
  -m "Test: npm run build:wasm" \
  -m "Test: npm test -- src/daemon-web/gateways.test.ts src/daemon-web/wasm-semantics.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 7: Select a Gateway and manage sessions

**Files:**
- Create: `products/gitim/frontend/src/lib/gateway-client.ts`
- Create: `products/gitim/frontend/src/lib/gateway-client.test.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [ ] **Step 1: Write failing selection and session tests**

Create `gateway-client.test.ts`:

```ts
it("prefers the last successful compatible runtime", async () => {
  const client = new GatewayClient({ fetcher, storage, now });
  storage.setItem("gitim-gateway:last:ws_phone", "runtime-b");
  fetcher.mockHello("runtime-a", 20);
  fetcher.mockHello("runtime-b", 40);
  const selected = await client.connect(contextWith([gatewayA, gatewayB]));
  expect(selected.runtimeId).toBe("runtime-b");
});

it("verifies runtime identity before token exchange", async () => {
  fetcher.mockHelloResponse(gatewayA.endpoint, { runtime_id: "runtime-x" });
  await client.connect(contextWith([gatewayA]));
  expect(fetcher.sessionRequests()).toHaveLength(0);
});

it("refreshes an expired session with the current browser token", async () => {
  const session = await client.connect(contextWith([gatewayA]));
  now.advance(601_000);
  await client.resolve(session, assetRef);
  expect(fetcher.sessionRequests()).toHaveLength(2);
});
```

- [ ] **Step 2: Run and verify tests fail**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/gateway-client.test.ts
```

Expected: FAIL because `GatewayClient` does not exist.

- [ ] **Step 3: Implement Gateway selection**

Create `gateway-client.ts` with this stable public surface:

```ts
export interface GatewayContext {
  browserWorkspaceId: string;
  workspaceIdentity: string;
  githubToken: string;
  descriptors: GatewayDescriptor[];
}

export interface ActiveGateway {
  runtimeId: string;
  endpoint: string;
  capabilities: Set<GatewayCapability>;
  sessionToken: string;
  canWrite: boolean;
  expiresAt: number;
}

export class GatewayClient {
  async connect(context: GatewayContext): Promise<ActiveGateway | null>;
  async resolve(gateway: ActiveGateway, asset: AssetRef): Promise<Blob>;
  async upload(gateway: ActiveGateway, files: File[]): Promise<UploadedAsset[]>;
  clear(workspaceId: string): void;
}
```

`connect` validates `hello.runtime_id`, filters by required capability, sends the
GitHub token only after identity match, and stores only the winning runtime ID in
localStorage. Session tokens remain in memory. Probe the last-successful candidate
first, then remaining descriptors in priority order with concurrency two. Pin the
winner until transport failure or 401.

- [ ] **Step 4: Add bounded failover**

On network failure, clear the active session, exclude the failed Runtime for the
current attempt, connect once to the next eligible descriptor, and retry a GET or
HEAD exactly once. Upload retries require the user action because the first
request may have completed; content-addressed dedupe keeps a manual retry safe.

- [ ] **Step 5: Verify Gateway client behavior**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/gateway-client.test.ts
```

Expected: candidate ordering, identity-before-token, permission, expiry, and
bounded failover tests PASS.

- [ ] **Step 6: Commit Gateway client**

```bash
git add products/gitim/frontend/src/lib/gateway-client.ts \
  products/gitim/frontend/src/lib/gateway-client.test.ts \
  products/gitim/frontend/src/lib/client.ts
git commit -m "feat(web): connect browser workspaces to gateways" \
  -m "Test: npm test -- src/lib/gateway-client.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 8: Enable Browser/WASM attachment render and upload

**Files:**
- Modify: `products/gitim/frontend/src/lib/client.ts`
- Modify: `products/gitim/frontend/src/lib/client.assets.test.ts`
- Modify: `products/gitim/frontend/src/components/chat/asset-fragment.tsx`
- Modify: `products/gitim/frontend/src/components/chat/message-body.test.tsx`
- Modify: `products/gitim/frontend/src/hooks/use-attachment-draft-store.test.ts`

- [ ] **Step 1: Write failing Browser Gateway asset tests**

Add tests that activate a Browser workspace with one Gateway descriptor, upload
a `File`, receive a canonical ref, fetch an image Blob with Authorization, and
revoke its object URL after unmount. Keep the existing no-Gateway test and expect
the Runtime-required metadata card.

```ts
it("uploads browser attachments through an assets.write gateway", async () => {
  await activateBrowserGateway({ canWrite: true });
  const response = await uploadAssets("browser-ws", [
    new File(["png"], "phone.png", { type: "image/png" }),
  ]);
  expect(response.ok).toBe(true);
  expect(response.data?.assets[0].asset_ref).toMatch(/^<\^v1\//);
});
```

- [ ] **Step 2: Run and verify tests fail**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/client.assets.test.ts src/components/chat/message-body.test.tsx
```

Expected: Browser mode still returns `runtime_required` and tests FAIL.

- [ ] **Step 3: Route Browser uploads through the active Gateway**

In `client.ts`, replace the unconditional local-mode asset failure with:

```ts
if (isLocalMode()) {
  const gateway = await activeGatewayForWorkspace(workspace);
  if (!gateway) return ASSET_LOCAL_UNAVAILABLE;
  if (!gateway.canWrite) {
    return { ok: false, error: "Repository write access is required.", error_code: "forbidden" };
  }
  return gatewayClient.upload(gateway, files);
}
```

Preserve current upload limits, draft generations, canonical-ref validation, and
send-after-upload behavior.

- [ ] **Step 4: Render authenticated Gateway images**

In `asset-fragment.tsx`, add a Browser Gateway source that calls
`gatewayClient.resolve`, creates `URL.createObjectURL(blob)`, and revokes the URL
on retry, asset change, workspace change, and unmount. Keep current Runtime URL
rendering unchanged. A failed Gateway resolution enters the existing unavailable
card with Retry; no Gateway keeps the metadata-only Runtime-required card.

- [ ] **Step 5: Verify frontend assets**

Run:

```bash
cd products/gitim/frontend
npm test -- src/lib/client.assets.test.ts \
  src/components/chat/message-body.test.tsx \
  src/hooks/use-attachment-draft-store.test.ts
npm run lint
npm run build
```

Expected: targeted tests, lint, and build PASS.

- [ ] **Step 6: Commit Browser attachment support**

```bash
git add products/gitim/frontend/src/lib/client.ts \
  products/gitim/frontend/src/lib/client.assets.test.ts \
  products/gitim/frontend/src/components/chat/asset-fragment.tsx \
  products/gitim/frontend/src/components/chat/message-body.test.tsx \
  products/gitim/frontend/src/hooks/use-attachment-draft-store.test.ts
git commit -m "feat(web): resolve browser assets through gateways" \
  -m "Test: npm test -- src/lib/client.assets.test.ts src/components/chat/message-body.test.tsx src/hooks/use-attachment-draft-store.test.ts" \
  -m "Test: npm run lint" \
  -m "Test: npm run build" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 9: Complete mobile E2E and operational documentation

**Files:**
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`
- Create: `docs/plans/mobile-gateway-discovery/02-e2e-evidence.md`
- Modify: `docs/plans/mobile-gateway-discovery/00-design.md`

- [ ] **Step 1: Add deterministic Playwright coverage**

Extend `mobile-layout.spec.ts` with a mock HTTPS Gateway that verifies:

```ts
test("mobile browser discovers, authenticates, uploads, and resolves", async ({ page }) => {
  await seedBrowserWorkspaceWithGateway(page, gatewayDescriptor);
  await openMobileWorkspace(page);
  await pasteFixture(page, "fixtures/fleet-assets.png");
  await expect(page.getByAltText("fleet-assets.png")).toBeVisible();
  expect(gateway.requests.session).toHaveLength(1);
  expect(gateway.requests.resolve[0].authorization).toMatch(/^Bearer /);
});
```

Add companion cases for preferred-Gateway outage, read-only upload rejection,
token expiry, identity mismatch before token exchange, and all-Gateways-offline
metadata fallback.

- [ ] **Step 2: Run scoped E2E**

Run:

```bash
cd products/gitim/frontend
npm run test:e2e -- --grep "mobile browser"
```

Expected: all mobile Gateway E2E cases PASS.

- [ ] **Step 3: Run a real Tailnet smoke test**

On two operator-owned nodes and one Tailscale-connected phone:

```text
1. Enable Gateway publication on the preferred Runtime.
2. Configure Tailscale HTTPS forwarding to its dedicated loopback listener.
3. Open the GitHub Browser workspace on the phone.
4. Paste and send one PNG.
5. Resolve the PNG through a second Runtime after stopping the origin.
6. Confirm the phone receives no Fleet peer URL and the replica SHA-256 matches.
```

Record runtime IDs, Git commit IDs, response status, SHA-256, and screenshot paths
in `02-e2e-evidence.md`; redact GitHub tokens, session tokens, tailnet names, and
peer addresses.

- [ ] **Step 4: Run the final regression set**

Run:

```bash
env -u HTTP_PROXY -u HTTPS_PROXY -u ALL_PROXY \
  -u http_proxy -u https_proxy -u all_proxy \
  cargo test --locked
cd products/gitim/frontend
npm test
npm run lint
npm run build
npm run test:e2e
```

Expected: Rust workspace tests, frontend tests, lint, build, and mobile/sidebar
E2E all PASS.

- [ ] **Step 5: Audit acceptance criteria and write evidence**

Create `02-e2e-evidence.md` with one row per acceptance criterion from
`00-design.md`, the exact automated test name or live observation, the tested
commit, and PASS/FAIL. Keep the design status unchanged until every row passes.

- [ ] **Step 6: Commit final evidence**

```bash
git add products/gitim/frontend/e2e/mobile-layout.spec.ts \
  docs/plans/mobile-gateway-discovery/00-design.md \
  docs/plans/mobile-gateway-discovery/02-e2e-evidence.md
git commit -m "test(gateway): verify mobile asset routing" \
  -m "Test: cargo test --locked" \
  -m "Test: npm test" \
  -m "Test: npm run lint" \
  -m "Test: npm run build" \
  -m "Test: npm run test:e2e" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

## Self-review checklist

- [ ] Every `00-design.md` acceptance criterion maps to a test or live E2E step.
- [ ] Gateway descriptors are daemon-written and low-churn.
- [ ] Runtime identity is checked before any GitHub token is sent.
- [ ] GitHub tokens are redacted and dropped after session exchange.
- [ ] Sessions are workspace- and permission-scoped, memory-only, and expiring.
- [ ] The mobile router contains no administration routes.
- [ ] Browser/WASM retains metadata-only behavior with no eligible Gateway.
- [ ] Fleet transfer remains demand-driven and content-verified.
- [ ] Existing Runtime-backed attachment behavior is unchanged.
- [ ] WASM package output is rebuilt and committed.
- [ ] Final verification uses the tested commit and records exact evidence.
