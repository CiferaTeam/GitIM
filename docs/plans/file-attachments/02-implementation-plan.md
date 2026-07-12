# File Attachments v1 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use
> `superpowers:executing-plans` to implement this plan task-by-task in the
> existing `.codex/file-attachments` worktree. Every behavior change follows
> RED → GREEN → scoped verification before the next task.

**Goal:** Add safe ordinary-IM image/file upload, rendering, download, Agent CLI,
and content-addressed Fleet resolution without putting binary bytes in Git.

**Architecture:** Git stores canonical AssetRefs containing SHA-256 identity and
an origin Runtime routing hint. Each Runtime owns a workspace-bound object store
outside Git clones; a local miss resolves through workspace-matched Fleet peers,
verifies the complete object, and persists a demand-driven replica. The shared
React composer uploads before using existing send callbacks, while browser/WASM
mode renders metadata only.

**Tech Stack:** Stable Rust, Axum 0.8 multipart, Tokio streaming, reqwest 0.12,
SHA-256, tempfile/fs2, infer/imagesize, Tower HTTP ServeFile, clap, React 19,
TypeScript 6, Zustand 5, Vitest, Playwright, wasm-bindgen, Kimi WebBridge.

---

## Milestones and file boundaries

1. Protocol: shared fixtures plus `gitim-core` AssetRef/LinkKind.
2. Fleet identity: atomic `runtime.json` mutation and verified remote UUIDs.
3. Runtime data plane: focused `assets/{error,inspect,store,resolver,http}.rs`
   modules; existing `http.rs` only owns state/router integration.
4. Agent surface: `cli/cmd_asset.rs`, streaming client methods, and prompt text.
5. Frontend: `asset-ref.ts`, one draft store, one shared composer strip, and one
   message asset renderer; existing channel/DM/card send callbacks stay intact.
6. Compatibility: regenerate checked-in WASM, then automated and live E2E.

Each task's **Files** block is the authoritative complete inventory. Files are
split by one responsibility; no asset persistence, Fleet transfer, or UI state
logic is added directly to the existing giant Runtime/React orchestration files.

### Task 1: Canonical AssetRef grammar and shared fixtures

**Files:**

- Create: `testdata/protocol/asset_refs_v1.json`
- Create: `crates/gitim-core/src/types/asset.rs`
- Create: `crates/gitim-core/tests/asset_ref_test.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`
- Modify: `crates/gitim-core/Cargo.toml`

- [x] **Step 1: Write the shared fixture corpus**

The JSON schema is data, not executable test logic:

```json
{
  "valid": [
    {
      "raw": "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=fleet-assets.png&type=image%2Fpng&size=184203&width=1600&height=900>",
      "name": "fleet-assets.png",
      "media_type": "image/png",
      "size": 184203,
      "width": 1600,
      "height": 900
    },
    {
      "raw": "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855?name=%E6%8A%A5%E5%91%8A.txt&type=application%2Foctet-stream&size=0>",
      "name": "报告.txt",
      "media_type": "application/octet-stream",
      "size": 0,
      "width": null,
      "height": null
    }
  ],
  "invalid": [
    "<^v2/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=a&type=image%2Fpng&size=1>",
    "<^v1/3C6A295E-744A-41DC-BA60-5C21BB94E5A2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=a&type=image%2Fpng&size=1>",
    "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8F2C4D7D7E931A62C18F6F24C8E388D72524D4C4CD6F88E9538F7D4A66C72A88?name=a&type=image%2Fpng&size=1>",
    "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?type=image%2Fpng&name=a&size=1>",
    "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=..%2Fsecret&type=image%2Fpng&size=1>",
    "<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=a&type=image%2Fpng&size=1&width=1>"
  ]
}
```

- [x] **Step 2: Write failing Rust fixture and boundary tests**

```rust
#[test]
fn shared_fixture_round_trips_canonical_refs() {
    let fixture: Fixture = serde_json::from_str(include_str!(
        "../../../testdata/protocol/asset_refs_v1.json"
    ))
    .expect("fixture JSON");
    for case in fixture.valid {
        let parsed: AssetRef = case.raw.parse().expect("valid fixture");
        assert_eq!(parsed.to_string(), case.raw);
        assert_eq!(parsed.name, case.name);
        assert_eq!(parsed.media_type, case.media_type);
        assert_eq!(parsed.size, case.size);
    }
    for raw in fixture.invalid {
        assert!(raw.parse::<AssetRef>().is_err(), "accepted {raw}");
    }
}

#[test]
fn encoded_reference_limit_is_authoritative() {
    let mut asset = sample_asset();
    asset.name = "界".repeat(100);
    assert!(matches!(asset.validate(), Err(AssetRefError::ReferenceTooLong)));
}
```

- [x] **Step 3: Run tests and verify RED**

Run:

```bash
cargo test -p gitim-core --test asset_ref_test --locked
```

Expected: compilation fails because `gitim_core::types::AssetRef` does not exist.

- [x] **Step 4: Implement the core type and canonical codec**

The public contract is exact:

```rust
pub const ASSET_REF_VERSION: u8 = 1;
pub const MAX_ASSET_BYTES: u64 = 50 * 1024 * 1024;
pub const MAX_ASSETS_PER_MESSAGE: usize = 10;
pub const MAX_ASSET_REQUEST_BYTES: u64 = 200 * 1024 * 1024;
pub const MAX_ASSET_FILENAME_BYTES: usize = 255;
pub const MAX_ASSET_MEDIA_TYPE_BYTES: usize = 127;
pub const MAX_ASSET_REF_BYTES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AssetRef {
    pub version: u8,
    pub origin_runtime_id: String,
    pub sha256: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

impl std::str::FromStr for AssetRef {
    type Err = AssetRefError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        let parsed = parse_components(raw)?;
        parsed.validate()?;
        if parsed.to_string() != raw {
            return Err(AssetRefError::NonCanonical);
        }
        Ok(parsed)
    }
}
```

`parse_components` accepts the exact ordered query forms
`name,type,size` and `name,type,size,width,height`. Decode with
`percent_decode_str(...).decode_utf8()`, reject controls, slash/backslash,
invalid lowercase MIME tokens, non-canonical UUID/hash, mismatched dimensions,
numeric overflow, and unknown/duplicate keys. `Display` percent-encodes every
byte outside RFC 3986 unreserved characters and checks the final 1024-byte limit
through `validate()`.

- [x] **Step 5: Run scoped core tests and verify GREEN**

Run:

```bash
cargo test -p gitim-core --test asset_ref_test --locked
cargo test -p gitim-core --lib link --locked
```

Expected: all tests pass.

- [x] **Step 6: Commit the protocol type**

```bash
git add testdata/protocol/asset_refs_v1.json crates/gitim-core/Cargo.toml \
  crates/gitim-core/src/types/asset.rs crates/gitim-core/src/types/mod.rs \
  crates/gitim-core/tests/asset_ref_test.rs Cargo.lock
git commit -m "feat(core): define canonical asset references" \
  -m "Test: cargo test -p gitim-core --test asset_ref_test --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 2: Link extraction, wire serialization, and WASM protocol surface

**Files:**

- Modify: `crates/gitim-core/src/types/link.rs`
- Modify: `crates/gitim-core/src/link.rs`
- Modify: `crates/gitim-core/tests/asset_ref_test.rs`
- Modify: `crates/gitim-daemon/src/handlers/serde.rs`
- Modify: `crates/gitim-wasm/src/lib.rs`

- [x] **Step 1: Add failing LinkKind wire tests**

```rust
#[test]
fn asset_link_serializes_additively() {
    let links = gitim_core::link::extract_links(&sample_asset().to_string());
    assert_eq!(links.len(), 1);
    let json = serde_json::to_value(&links[0]).expect("serialize link");
    assert_eq!(json["kind"]["kind"], "asset");
    assert_eq!(json["kind"]["asset"]["sha256"], sample_hash());
}

#[test]
fn invalid_asset_syntax_is_not_a_typed_link() {
    assert!(gitim_core::link::extract_links("<^v1/bad>").is_empty());
}
```

- [x] **Step 2: Run and verify RED**

Run: `cargo test -p gitim-core --test asset_ref_test asset_link --locked`

Expected: no link is extracted because `^` is not in `LINK_RE`.

- [x] **Step 3: Add the additive link variant and parser branch**

```rust
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkKind {
    Asset { asset: AssetRef },
}

static LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    crate::preconditions::regex_literal(r"<([#~!])([^>\n]+)>|<\^([^<>\n]+)>")
});

fn parse_asset(raw: &str) -> Option<LinkKind> {
    raw.parse::<AssetRef>()
        .ok()
        .map(|asset| LinkKind::Asset { asset })
}
```

Keep existing `#`, `~`, and `!` behavior byte-for-byte unchanged. Add the matching
daemon `link_to_json` arm with `{ kind: "asset", asset, raw }` so read and poll
responses remain exhaustive and additive. WASM's existing `extract_links` export
serializes the new variant automatically; use `wasm-bindgen-test` to exercise the
exported binding and inspect its returned `JsValue`.

- [x] **Step 4: Run core and WASM-native tests**

Run:

```bash
cargo test -p gitim-core --test asset_ref_test --locked
cargo test -p gitim-daemon handlers::serde::tests --locked
cargo test -p gitim-wasm --locked
wasm-pack test --node crates/gitim-wasm
cargo check --workspace --locked
```

Expected: all tests pass.

- [x] **Step 5: Commit the additive protocol link**

```bash
git add crates/gitim-core/src/types/link.rs crates/gitim-core/src/link.rs \
  crates/gitim-core/tests/asset_ref_test.rs crates/gitim-daemon/src/handlers/serde.rs \
  crates/gitim-wasm/src/lib.rs
git commit -m "feat(core): extract typed asset links" \
  -m "Test: cargo test -p gitim-core --test asset_ref_test --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 3: Atomic Runtime config and Fleet Runtime identity

**Files:**

- Modify: `crates/gitim-runtime/Cargo.toml`
- Modify: `crates/gitim-runtime/src/user_config.rs`
- Modify: `crates/gitim-runtime/src/fleet.rs`
- Modify: `crates/gitim-runtime/src/http.rs`
- Modify: `crates/gitim-runtime/src/update.rs`
- Modify: `crates/gitim-runtime/tests/config_schema.rs`
- Modify: `crates/gitim-runtime/tests/fleet_http.rs`

- [x] **Step 1: Write failing schema and atomic-mutation tests**

```rust
#[test]
fn legacy_fleet_entry_without_runtime_id_round_trips() {
    let cfg: UserConfig = serde_json::from_str(
        r#"{"runtime_id":"local","fleet_nodes":[{"node_id":"mini","base_url":"http://127.0.0.1:18068"}]}"#,
    )
    .expect("legacy config");
    assert_eq!(cfg.fleet_nodes[0].runtime_id, None);
}

#[test]
fn locked_mutations_preserve_both_updates() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("runtime.json");
    write_to(&UserConfig::default(), &path).expect("seed");
    std::thread::scope(|scope| {
        scope.spawn(|| mutate_at(&path, |cfg| cfg.listen_port = Some(19001)).unwrap());
        scope.spawn(|| mutate_at(&path, |cfg| {
            cfg.runtime_id = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2".into();
        }).unwrap());
    });
    let cfg = read_from(Some(&path));
    assert_eq!(cfg.listen_port, Some(19001));
    assert_eq!(cfg.runtime_id, "3c6a295e-744a-41dc-ba60-5c21bb94e5a2");
}
```

Add a child-process variant in the same integration target. The child is
selected with an environment variable and one test-harness filter so two OS
processes mutate different fields under the same `.lock` file; the parent waits
for both exits and asserts both values remain.

- [x] **Step 2: Write failing Fleet health/duplicate/backfill tests**

The mock remote exposes both `/health` and `/workspaces`:

```rust
async fn remote_health() -> Json<Value> {
    Json(json!({
        "service": "gitim-runtime",
        "runtime_id": "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
        "version": "0.9.3"
    }))
}

#[tokio::test]
async fn fleet_upsert_persists_verified_runtime_id() {
    let response = app.oneshot(post_fleet_node(&remote_url)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let cfg = user_config::read();
    assert_eq!(
        cfg.fleet_nodes[0].runtime_id.as_deref(),
        Some("3c6a295e-744a-41dc-ba60-5c21bb94e5a2")
    );
}
```

Also assert wrong `service`, malformed UUID, and a second alias with the same
Runtime ID return stable `invalid_fleet_node`/`duplicate_runtime_id` errors; a
legacy node remains active for SSE when health is unavailable.

- [x] **Step 3: Run and verify RED**

Run:

```bash
cargo test -p gitim-runtime --test config_schema --locked
cargo test -p gitim-runtime --test fleet_http --locked
```

Expected: `FleetNodeEntry.runtime_id`, `mutate_at`, and health discovery are
missing.

- [x] **Step 4: Implement locked atomic mutation**

Add:

```rust
pub fn mutate_at<T>(
    path: &Path,
    update: impl FnOnce(&mut UserConfig) -> T,
) -> std::io::Result<T> {
    let lock_path = path.with_extension("json.lock");
    let lock = OpenOptions::new().create(true).read(true).write(true).open(lock_path)?;
    lock.lock_exclusive()?;
    let mut cfg = read_from(Some(path));
    let output = update(&mut cfg);
    write_to_atomic(&cfg, path)?;
    lock.unlock()?;
    Ok(output)
}
```

`write_to_atomic` writes a `NamedTempFile` in the config directory, applies 0600
on Unix, flushes, persists over `runtime.json`, and best-effort syncs the parent
directory. Once rename commits, unlock or directory-sync warnings do not turn the
mutation into an ambiguous failure. Convert `write_listen_port`,
Runtime ID creation, workspace add/remove, and Fleet add/remove to locked
read-modify-write calls. The update/restart workspace snapshot uses the same
mutation primitive; no caller may retain the old `read(); mutate; write()`
sequence.

- [x] **Step 5: Implement Runtime-ID discovery and peer snapshots**

```rust
#[derive(Debug, Clone, Deserialize)]
struct RemoteHealth {
    service: String,
    runtime_id: String,
}

pub async fn fetch_remote_runtime_id(base_url: &str) -> Result<String, String> {
    let health: RemoteHealth = health_client()
        .get(format!("{}/health", base_url.trim_end_matches('/')))
        .send().await.map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| e.to_string())?
        .json().await.map_err(|e| e.to_string())?;
    if health.service != "gitim-runtime" || uuid::Uuid::parse_str(&health.runtime_id).is_err() {
        return Err("remote /health is not a GitIM Runtime with a valid UUID".into());
    }
    Ok(health.runtime_id)
}
```

Fleet upsert discovers identity before persistence, rejects a duplicate on a
different alias, and returns the enriched entry. Startup recovery performs a
best-effort one-shot discovery for legacy entries; the first asset resolution
retries it. A successful backfill updates the live map and locked config only if
the alias/base URL still match the snapshot.

A per-Runtime Fleet transition mutex serializes each short disk mutation plus
live-map apply for upsert, delete, and backfill; remote HTTP stays outside it and
the main `RuntimeState` guard is never held during filesystem I/O. UUID duplicate
checks use canonical parsed values. Recovery normalizes entries and deterministically
suppresses duplicate aliases or Runtime UUIDs. Before a new alias commits, every
other active legacy alias must resolve its Runtime identity; an unavailable identity
returns `fleet_identity_unresolved`. Duplicate backfills persist the canonical ID
on both entries and keep the earliest configured, normalized, valid, currently
active alias as winner, independent of network completion order. Invalid or
suppressed config rows cannot evict the only live candidate. Legacy identity
discovery runs with concurrency 8. `/health` bodies are capped at 64 KiB for both
fixed and chunked responses.

Expose a pure snapshot function returning every workspace-matched peer with
`node_id`, optional `runtime_id`, `base_url`, and mapped remote slug. The
resolver sorts this snapshot and never holds `RuntimeState` across await.
Identical peer rows are deduplicated after sorting.

- [x] **Step 6: Run scoped tests and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --test config_schema --locked
cargo test -p gitim-runtime --test fleet_http --locked
cargo test -p gitim-runtime cli::cmd_fleet --locked
```

Expected: all tests pass, including legacy JSON.

- [x] **Step 7: Commit config/Fleet identity**

```bash
git add crates/gitim-runtime/Cargo.toml crates/gitim-runtime/src/user_config.rs \
  crates/gitim-runtime/src/fleet.rs crates/gitim-runtime/src/http.rs \
  crates/gitim-runtime/src/update.rs \
  crates/gitim-runtime/tests/config_schema.rs crates/gitim-runtime/tests/fleet_http.rs Cargo.lock
git commit -m "feat(fleet): persist verified runtime identities" \
  -m "Test: cargo test -p gitim-runtime --test fleet_http --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 4: Store namespace, inspection, quotas, and recovery

**Files:**

- Create: `crates/gitim-runtime/src/assets/mod.rs`
- Create: `crates/gitim-runtime/src/assets/error.rs`
- Create: `crates/gitim-runtime/src/assets/inspect.rs`
- Create: `crates/gitim-runtime/src/assets/store.rs`
- Create: `crates/gitim-runtime/tests/assets_store.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`
- Modify: `crates/gitim-runtime/src/http.rs`
- Modify: `crates/gitim-runtime/Cargo.toml`

- [x] **Step 1: Add dependencies with minimal features**

Use:

```toml
axum = { version = "0.8", features = ["multipart"] }
fs2 = "0.4"
infer = "0.19"
imagesize = { version = "0.15", default-features = false, features = ["png", "jpeg", "gif", "webp", "heif"] }
mime = "0.3"
tokio-util = { workspace = true, features = ["io"] }
tower-http = { version = "0.6", features = ["cors", "fs"] }
```

Extend reqwest features with `multipart`. Do not add the full image decoder
crate; `infer` and bounded `imagesize` cover the approved raster formats.

- [x] **Step 2: Write failing store binding and inspection tests**

```rust
#[test]
fn store_reuse_with_different_binding_quarantines_old_namespace() {
    let root = temp_workspace();
    let first = AssetStore::open(root.path(), "github:github.com/a/one", limits()).unwrap();
    first.put_bytes(b"one", upload("one.txt")).unwrap();
    let second = AssetStore::open(root.path(), "github:github.com/a/two", limits()).unwrap();
    assert_eq!(second.usage().objects, 0);
    assert_eq!(orphaned_asset_trees(root.path()).len(), 1);
}

#[test]
fn inspection_uses_magic_not_extension() {
    let inspected = inspect_bytes(PNG_1X1, "actually.html").unwrap();
    assert_eq!(inspected.media_type, "image/png");
    assert_eq!((inspected.width, inspected.height), (Some(1), Some(1)));
    assert!(inspected.inline_safe);
}
```

Add fixtures for JPEG/GIF/WebP/AVIF, SVG/HTML forced download, unknown bytes,
0-byte content, malformed image headers, and dimensions outside `u32`.

- [x] **Step 3: Write failing quota/recovery tests**

```rust
#[test]
fn quota_counts_origin_and_replica_objects_without_counting_dedupe_twice() {
    let store = small_quota_store(6);
    store.put_bytes(b"abc", local_upload("a")).unwrap();
    store.put_bytes(b"abc", fleet_replica(origin())).unwrap();
    assert_eq!(store.usage(), AssetUsage { bytes: 3, objects: 1 });
    let err = store.put_bytes(b"defg", local_upload("b")).unwrap_err();
    assert!(matches!(err, AssetError::QuotaExceeded { .. }));
}

#[test]
fn startup_cleanup_keeps_recent_temp_and_removes_stale_owned_temp() {
    let store = prepared_store();
    let recent = store.create_owned_temp().unwrap();
    let stale = store.create_owned_temp().unwrap();
    set_mtime_older_than(&stale, Duration::from_secs(25 * 60 * 60));
    store.recover().unwrap();
    assert!(recent.exists());
    assert!(!stale.exists());
}
```

Also cover object-only, sidecar-only, invalid sidecar, binding manifest partial
write, exact quota, reserve failure, permissions, and a non-GitIM file in `tmp/`
that cleanup must preserve.

- [x] **Step 4: Run and verify RED**

Run: `cargo test -p gitim-runtime --test assets_store --locked`

Expected: the `assets` module and store types do not exist.

- [x] **Step 5: Implement service state and typed errors**

```rust
pub struct AssetService {
    pub upload_slots: Arc<Semaphore>,
    pub peer_slots: Arc<Semaphore>,
    usage: Mutex<HashMap<PathBuf, AssetUsage>>,
    pub store_failures: AtomicU64,
    pub hash_mismatches: AtomicU64,
    pub fleet_fetch_failures: AtomicU64,
    pub limits: AssetLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AssetUsage { pub bytes: u64, pub objects: u64 }

#[derive(Debug, thiserror::Error)]
pub enum AssetError {
    Invalid(String),
    TooLarge { limit: u64 },
    RequestTooLarge { limit: u64 },
    TooMany { limit: usize },
    QuotaExceeded { used: u64, quota: u64 },
    Store(std::io::Error),
    Missing,
    OriginUnavailable,
    HashMismatch,
    PeerInvalid(String),
    ForbiddenOrigin,
}
```

Implement one `status_code()` and `error_code()` mapping in `error.rs`; HTTP and
CLI consume it instead of duplicating matches.

- [x] **Step 6: Implement binding, recovery, inspection, and usage scan**

`AssetStore::open(workspace_root, binding, limits)` validates/creates
`store.json`, quarantines a mismatched namespace with an atomic rename, creates
0700 directories, and scans `objects/sha256/*/*` once. `recover()` removes only
owned `gitim-asset-*.tmp` files older than 24 hours and reconciles partial
object/sidecar state. Sidecars store `object_modified_ns`; any length/mtime
change forces re-hash and inspection before the object is returned.

Before each streamed write, check both `used + in_flight <= quota` and
`available_space - incoming_chunk >= max(configured_min_free, total_space / 20)`.
AssetService's two-slot upload semaphore bounds simultaneous multipart requests.

- [x] **Step 7: Run store tests and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --test assets_store --locked
cargo test -p gitim-runtime git_config --locked
```

Expected: all store, binding, inspection, quota, and recovery tests pass.

- [x] **Step 8: Commit store foundation**

```bash
git add crates/gitim-runtime/Cargo.toml crates/gitim-runtime/src/lib.rs \
  crates/gitim-runtime/src/http.rs crates/gitim-runtime/src/assets \
  crates/gitim-runtime/tests/assets_store.rs Cargo.lock
git commit -m "feat(runtime): add workspace-bound asset store" \
  -m "Test: cargo test -p gitim-runtime --test assets_store --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 5: Streaming staging, atomic dedupe, and cross-process singleflight

**Files:**

- Modify: `crates/gitim-runtime/src/assets/store.rs`
- Modify: `crates/gitim-runtime/tests/assets_store.rs`

- [x] **Step 1: Write failing streaming and batch tests**

```rust
#[tokio::test]
async fn stage_stream_hashes_without_buffering_and_preserves_order() {
    let store = prepared_store();
    let chunks = stream::iter(vec![Ok(Bytes::from_static(b"ab")), Ok(Bytes::from_static(b"c"))]);
    let staged = store.stage_stream("a.txt", chunks, &mut RequestBudget::default()).await.unwrap();
    assert_eq!(staged.size, 3);
    assert_eq!(staged.sha256, sha256_hex(b"abc"));
    assert_eq!(staged.media_type, "application/octet-stream");
}

#[tokio::test]
async fn invalid_late_file_persists_no_batch_refs_and_cleans_all_temps() {
    let store = prepared_store();
    let first = store.stage_bytes("ok.txt", b"ok").await.unwrap();
    let second = store.stage_bytes("too-big.bin", &[0; 7]).await.unwrap_err();
    drop(second);
    drop(first);
    assert_eq!(owned_temp_files(store.root()).count(), 0);
    assert_eq!(store.usage().objects, 0);
}
```

Use a test-only `max_file_bytes = 6` limit rather than allocating 50 MiB.

- [x] **Step 2: Write failing dedupe/corruption/lock tests**

```rust
#[tokio::test]
async fn dedupe_replaces_same_length_corrupt_existing_object() {
    let store = prepared_store();
    let original = store.put_bytes(b"good", local_upload("a")).await.unwrap();
    std::fs::write(store.object_path(&original.sha256), b"evil").unwrap();
    let repaired = store.put_bytes(b"good", local_upload("b")).await.unwrap();
    assert_eq!(repaired.sha256, original.sha256);
    assert_eq!(std::fs::read(store.object_path(&original.sha256)).unwrap(), b"good");
}

#[tokio::test]
async fn simultaneous_resolvers_share_one_cross_process_hash_lock() {
    let child_a = spawn_lock_child(&workspace, &hash, "hold");
    wait_for_lock_marker(&workspace);
    let child_b = spawn_lock_child(&workspace, &hash, "measure");
    assert!(child_b.wait_with_output().unwrap().stdout.starts_with(b"blocked="));
    assert!(child_a.wait().unwrap().success());
}
```

The child helper records elapsed acquisition time; assert the second process
cannot enter until the first releases. Also test lock release on task error.

- [x] **Step 3: Run and verify RED**

Run: `cargo test -p gitim-runtime --test assets_store streaming --locked`

Expected: staging and hash-lock APIs are absent.

- [x] **Step 4: Implement streaming staging**

Use an `AssetStager` that owns a tempfile path, Tokio file, incremental `Sha256`,
byte count, first 64 KiB of inspection bytes, and a quota reservation. Its
`write_chunk` checks file/request/quota/free-space limits before `write_all`.
`finish` flushes, converts the file back to a closed standard handle, inspects
the tempfile, and returns `StagedAsset`; Drop removes unfinished temp paths.

The batch path stages and validates every file and fully formats every AssetRef
before calling `persist_batch`. A persistence error may leave an unreferenced
valid dedupe object but returns no refs; retry converges by hash.

- [x] **Step 5: Implement no-clobber persistence and hash locks**

```rust
pub struct HashLock { file: std::fs::File }

impl HashLock {
    pub async fn acquire(store: &AssetStore, hash: &str) -> Result<Self, AssetError> {
        let path = store.lock_path(hash)?;
        let file = tokio::task::spawn_blocking(move || {
            let file = OpenOptions::new().create(true).read(true).write(true).open(path)?;
            file.lock_exclusive()?;
            Ok::<_, std::io::Error>(file)
        }).await.map_err(join_error)??;
        Ok(Self { file })
    }
}

impl Drop for HashLock {
    fn drop(&mut self) { let _ = self.file.unlock(); }
}
```

Under the lock, re-check any existing object. A valid existing hash is a dedupe
hit; an invalid existing object/sidecar pair is atomically moved to quarantine.
Persist the tempfile with no-clobber semantics, then atomically persist the
sidecar. Update usage only when this call created a new object.

- [x] **Step 6: Run and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --test assets_store --locked
cargo test -p gitim-runtime assets::store --locked
```

Expected: all tests pass with no `tmp/` residue.

- [x] **Step 7: Commit streaming persistence**

```bash
git add crates/gitim-runtime/src/assets/store.rs crates/gitim-runtime/tests/assets_store.rs
git commit -m "feat(runtime): stream and deduplicate asset writes" \
  -m "Test: cargo test -p gitim-runtime --test assets_store --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 6: Local upload/serve HTTP and browser boundary

**Files:**

- Create: `crates/gitim-runtime/src/assets/http.rs`
- Create: `crates/gitim-runtime/tests/assets_http.rs`
- Modify: `crates/gitim-runtime/src/assets/mod.rs`
- Modify: `crates/gitim-runtime/src/http.rs`

- [x] **Step 1: Write failing browser guard tests**

```rust
#[tokio::test]
async fn malicious_origin_is_rejected_before_upload_body_is_consumed() {
    let request = multipart_request("/workspaces/room/assets", PNG_1X1)
        .header("origin", "https://evil.example")
        .header("sec-fetch-site", "cross-site");
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(asset_objects(&workspace).count(), 0);
}

#[tokio::test]
async fn node_local_object_rejects_every_browser_context() {
    let request = Request::builder()
        .uri(object_uri(&hash))
        .header("origin", "https://gitim.io")
        .header("sec-fetch-site", "cross-site")
        .body(Body::empty()).unwrap();
    assert_eq!(app.oneshot(request).await.unwrap().status(), StatusCode::FORBIDDEN);
}
```

Add table-driven cases for exact production origins, loopback dev origins,
explicit `GITIM_WEB_ORIGINS`, `Origin: null`, Fetch Metadata without Origin,
Origin-less CLI/peer requests, and the exact GET navigation tuple
`navigate/document/?1`. Upload must reject the navigation exception.

- [x] **Step 2: Write failing upload and local response tests**

```rust
#[tokio::test]
async fn upload_returns_canonical_ref_and_local_get_supports_range_etag_and_head() {
    let upload = app.clone().oneshot(allowed_upload(PNG_1X1, "pixel.png")).await.unwrap();
    assert_eq!(upload.status(), StatusCode::OK);
    let asset = json_body(upload).await["assets"][0].clone();
    assert!(asset["ref"].as_str().unwrap().parse::<AssetRef>().is_ok());

    let get = app.clone().oneshot(get_resolve(&asset, Some("bytes=0-7"))).await.unwrap();
    assert_eq!(get.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(get.headers()["content-type"], "image/png");
    assert_eq!(get.headers()["x-content-type-options"], "nosniff");
    assert_eq!(body_bytes(get).await.len(), 8);

    let not_modified = app.oneshot(get_with_if_none_match(&asset)).await.unwrap();
    assert_eq!(not_modified.status(), StatusCode::NOT_MODIFIED);
}
```

Cover repeated file fields, unknown fields, 0/10/11 files, exact and over-limit
bytes, aggregate limit, encoded ref overflow, forced SVG/HTML download, Unicode
RFC 5987 filename, HEAD with empty body, suffix/invalid/multi ranges, and unknown
workspace/binding failures.

- [x] **Step 3: Run and verify RED**

Run: `cargo test -p gitim-runtime --test assets_http --locked`

Expected: asset routes return 404.

- [x] **Step 4: Implement middleware and upload route**

Build three nested route groups so security runs before body extraction:

```rust
pub fn router() -> Router<SharedRuntimeState> {
    let upload = Router::new()
        .route("/assets", post(upload_assets))
        .route_layer(middleware::from_fn(guard_upload_browser))
        .layer(DefaultBodyLimit::max(MAX_UPLOAD_HTTP_BYTES));
    let resolve = Router::new()
        .route("/assets/resolve/{origin}/{hash}", get(resolve_asset).head(resolve_asset))
        .route_layer(middleware::from_fn(guard_resolve_browser));
    let objects = Router::new()
        .route("/assets/objects/{hash}", get(local_object).head(local_object))
        .route_layer(middleware::from_fn(reject_browser_context));
    upload.merge(resolve).merge(objects)
}
```

`upload_assets` obtains the two-slot permit, snapshots workspace path/binding and
Runtime ID, streams repeated `file` fields in order, validates the full batch,
persists, and returns typed JSON. It never holds `RuntimeState` during multipart
or filesystem awaits.

- [x] **Step 5: Implement local serving with Tower ServeFile**

Local lookup validates hash, sidecar, length, and mtime. Exact SHA ETag match
returns 304 before opening the file. Otherwise call
`ServeFile::new_with_mime(path, &mime).oneshot(request)` and overwrite only the
approved `Content-Type`, browser-no-store or peer-immutable cache policy, ETag,
nosniff, and safe Content-Disposition headers. Non-inline MIME always sets
attachment; `download=1` forces attachment. Do not implement a custom Range
parser.

- [x] **Step 6: Expose service health and recover stores**

Add AssetService to `RuntimeState::default()`. Workspace recovery and successful
`/git/init` call store recovery with a workspace binding and cache usage. Health
adds the three counters plus `asset_bytes`, `asset_objects`, and quota to each
workspace entry without scanning.

- [x] **Step 7: Run and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --test assets_http --locked
cargo test -p gitim-runtime --test http_workspaces --locked
cargo test -p gitim-runtime --test runtime_http --locked
```

Expected: all tests pass; non-asset route body limits remain unchanged.

- [x] **Step 8: Commit local HTTP assets**

```bash
git add crates/gitim-runtime/src/assets/http.rs crates/gitim-runtime/src/assets/mod.rs \
  crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/assets_http.rs
git commit -m "feat(runtime): expose secure asset HTTP routes" \
  -m "Test: cargo test -p gitim-runtime --test assets_http --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 7: Fleet resolver, verified replicas, and bounded fallback

**Files:**

- Create: `crates/gitim-runtime/src/assets/resolver.rs`
- Create: `crates/gitim-runtime/tests/assets_fleet.rs`
- Modify: `crates/gitim-runtime/src/assets/mod.rs`
- Modify: `crates/gitim-runtime/src/assets/http.rs`
- Modify: `crates/gitim-runtime/src/fleet.rs`

- [x] **Step 1: Write failing origin and offline-replica integration test**

```rust
#[tokio::test]
async fn remote_get_verifies_persists_and_survives_origin_shutdown() {
    let peer = MockPeer::with_object("remote-room", PNG_1X1).await;
    let (app, workspace) = local_runtime_with_peer(&peer).await;
    let asset = peer.asset_ref();

    let first = app.clone().oneshot(resolve_request(&asset)).await.unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(body_bytes(first).await, PNG_1X1);
    assert_eq!(peer.object_get_count(), 1);
    assert!(replica_path(&workspace, asset.sha256()).exists());

    peer.shutdown().await;
    let second = app.oneshot(resolve_request(&asset)).await.unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(body_bytes(second).await, PNG_1X1);
}
```

- [x] **Step 2: Write failing fallback/integrity/budget tests**

```rust
#[tokio::test]
async fn fallback_finds_replica_after_third_sorted_alias() {
    let peers = four_sorted_peers_only_last_has_object(PNG_1X1).await;
    let app = runtime_with_peers(peers).await;
    let response = app.oneshot(resolve_request(&remote_ref())).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn concurrent_gets_issue_one_peer_object_get() {
    let peer = MockPeer::delayed_object(PNG_1X1).await;
    let app = Arc::new(runtime_with_peer(&peer).await);
    let results = futures::future::join_all((0..8).map(|_| {
        let app = app.clone();
        async move { app.clone().oneshot(resolve_request(&remote_ref())).await.unwrap() }
    })).await;
    assert!(results.iter().all(|r| r.status() == StatusCode::OK));
    assert_eq!(peer.object_get_count(), 1);
}
```

Add cases for local hash winning with a stale origin, origin 404 then fallback,
origin connection failure, wrong hash then clean fallback, all wrong hashes,
oversized/chunk-stalled/malformed responses, full budget expiry, peer GET never
receiving browser headers, quota failure, HEAD never persisting, and a legacy
entry whose `/health` backfill makes it the exact origin.

- [x] **Step 3: Run and verify RED**

Run: `cargo test -p gitim-runtime --test assets_fleet --locked`

Expected: remote resolve returns `asset_missing` because resolver does not exist.

- [x] **Step 4: Implement deterministic candidate snapshots**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetPeer {
    pub node_id: String,
    pub runtime_id: Option<String>,
    pub base_url: String,
    pub remote_workspace_slug: String,
}

pub fn order_peers(origin: &str, mut peers: Vec<AssetPeer>) -> Vec<AssetPeer> {
    peers.sort_by(|a, b| a.node_id.cmp(&b.node_id));
    peers.sort_by_key(|peer| (peer.runtime_id.as_deref() != Some(origin)) as u8);
    peers
}
```

Snapshot only mappings whose local slug and normalized workspace identity match.
Do not truncate the vector. A legacy peer remains eligible as fallback and is
probed for health when exact-origin lookup otherwise fails.

- [x] **Step 5: Implement GET singleflight and verified peer streaming**

```rust
pub async fn resolve_get(
    service: &AssetService,
    store: &AssetStore,
    origin: &str,
    hash: &str,
    peers: Vec<AssetPeer>,
) -> Result<StoredAsset, AssetError> {
    if let Some(local) = store.lookup(hash).await? { return Ok(local); }
    let _hash_lock = HashLock::acquire(store, hash).await?;
    if let Some(local) = store.lookup(hash).await? { return Ok(local); }
    let _peer_permit = service.peer_slots.acquire().await.map_err(closed)?;
    tokio::time::timeout(WHOLE_RESOLVE_TIMEOUT, async {
        fetch_candidates(service, store, origin, hash, peers).await
    }).await.map_err(|_| AssetError::OriginUnavailable)?
}
```

The origin candidate gets the first full GET. If it fails, HEAD every remaining
candidate with `buffer_unordered(8)` and the response-header timeout, then GET
HEAD-positive candidates one at a time. Each GET streams to store staging with a
5s connect, 10s header, 15s per-chunk idle, 90s candidate, and 120s whole budget.
Ignore peer MIME/name/dimensions; inspect the verified local temp. Discard every
failure temp. Error precedence is hash mismatch, peer invalid, unavailable, then
missing.

- [x] **Step 6: Implement HEAD without replica creation**

Local HEAD returns local metadata. A miss probes exact origin then all fallbacks
with concurrency eight, returns the first valid availability response, and never
takes the GET hash lock or writes an object/sidecar. Preserve 404 versus 503
semantics from peer outcomes.

- [x] **Step 7: Run and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --test assets_fleet --locked
cargo test -p gitim-runtime --test fleet_http --locked
cargo test -p gitim-runtime --test assets_http --locked
```

Expected: all local/Fleet tests pass; singleflight count is exactly one.

- [x] **Step 8: Commit Fleet resolution**

```bash
git add crates/gitim-runtime/src/assets/resolver.rs crates/gitim-runtime/src/assets/mod.rs \
  crates/gitim-runtime/src/assets/http.rs crates/gitim-runtime/src/fleet.rs \
  crates/gitim-runtime/tests/assets_fleet.rs
git commit -m "feat(runtime): resolve assets across fleet nodes" \
  -m "Test: cargo test -p gitim-runtime --test assets_fleet --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 8: Agent CLI put/get and prompt contract

**Files:**

- Create: `crates/gitim-runtime/src/cli/cmd_asset.rs`
- Create: `crates/gitim-runtime/tests/cli_asset.rs`
- Modify: `crates/gitim-runtime/src/cli/mod.rs`
- Modify: `crates/gitim-runtime/src/cli/http.rs`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`
- Modify: `crates/gitim-agent-provider/src/prompts.rs`
- Modify: `crates/gitim-runtime/tests/coordinator_prompt.rs`

- [x] **Step 1: Write failing clap and command tests**

```rust
#[test]
fn parses_asset_put_and_get() {
    let put = Args::try_parse_from([
        "gitim-runtime", "asset", "put", "--workspace", "room",
        "--file", "a.png", "--file", "b.pdf",
    ]).expect("asset put argv");
    assert!(matches_asset_put(put, "room", &["a.png", "b.pdf"]));

    let get = Args::try_parse_from([
        "gitim-runtime", "asset", "get", "--ref", VALID_REF,
        "--output", "download.png", "--force",
    ]).expect("asset get argv");
    assert!(matches_asset_get(get, "download.png", true));
}
```

Integration tests start the normal test router and assert single-workspace
default, multi-workspace ambiguity, repeatable file order, more than ten files,
canonical JSON stdout, destination default name, existing-file refusal, `--force`
replacement, wrong response hash cleanup, and no partial destination after a
transport abort.

- [x] **Step 2: Run and verify RED**

Run:

```bash
cargo test -p gitim-runtime --bin gitim-runtime asset --locked
cargo test -p gitim-runtime --test cli_asset --locked
```

Expected: clap rejects the unknown `asset` command.

- [x] **Step 3: Add clap surface and dispatch**

```rust
#[derive(Subcommand, Debug)]
enum AssetCommand {
    Put {
        #[arg(long)] workspace: Option<String>,
        #[arg(long = "file", required = true)] files: Vec<PathBuf>,
    },
    Get {
        #[arg(long)] workspace: Option<String>,
        #[arg(long = "ref")] asset_ref: String,
        #[arg(long)] output: Option<PathBuf>,
        #[arg(long)] force: bool,
    },
}
```

Add `Command::Asset { command: AssetCommand }` and route each variant through
`cmd_asset`; reuse `resolve_workspace` and the existing exit-code envelope.

- [x] **Step 4: Add streaming CLI HTTP methods**

`Client::post_files(path, files)` builds reqwest multipart parts from Tokio files
with known lengths and `Body::wrap_stream(ReaderStream)`. It uses the long request
timeout and existing JSON response classifier. `Client::get_binary(path)` returns
a success Response for streaming but collects/classifies non-success bodies with
the same `error_code` logic used by JSON verbs.

- [x] **Step 5: Implement put/get semantics**

`put` validates count, existence, regular-file status, filename/ref boundaries,
and each file length before HTTP; stdout is the Runtime JSON response.

`get` parses AssetRef with core, builds the local resolver URL, streams response
chunks into a `NamedTempFile` in the destination directory while hashing, checks
size/hash, flushes, then uses no-clobber persistence unless `--force`. On any
error the destination and pre-existing file are untouched.

- [x] **Step 6: Add prompt regression first, then text**

Failing assertion:

```rust
assert!(prompt.contains("gitim-runtime asset put"));
assert!(prompt.contains("gitim-runtime asset get"));
assert!(prompt.contains("copy the returned <^v1/...> ref into gitim send"));
```

Add the exact three-step put → send ref → get workflow to `default_gitim_api()`;
do not claim the Agent can read bytes directly from a message.

- [x] **Step 7: Run and verify GREEN**

Run:

```bash
cargo test -p gitim-runtime --bin gitim-runtime asset --locked
cargo test -p gitim-runtime --test cli_asset --locked
cargo test -p gitim-runtime --test coordinator_prompt --locked
cargo test -p gitim-agent-provider --locked
```

Expected: all tests pass.

- [x] **Step 8: Commit CLI and prompt**

```bash
git add crates/gitim-runtime/src/cli/cmd_asset.rs crates/gitim-runtime/src/cli/mod.rs \
  crates/gitim-runtime/src/cli/http.rs crates/gitim-runtime/src/bin/runtime.rs \
  crates/gitim-runtime/tests/cli_asset.rs crates/gitim-agent-provider/src/prompts.rs \
  crates/gitim-runtime/tests/coordinator_prompt.rs
git commit -m "feat(runtime): add agent asset put and get" \
  -m "Test: cargo test -p gitim-runtime --test cli_asset --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 9: Frontend AssetRef parser and Runtime client

**Files:**

- Create: `products/gitim/frontend/src/lib/asset-ref.ts`
- Create: `products/gitim/frontend/src/lib/asset-ref.test.ts`
- Create: `products/gitim/frontend/src/lib/client.assets.test.ts`
- Modify: `products/gitim/frontend/src/lib/message-parser.ts`
- Modify: `products/gitim/frontend/src/lib/message-parser.test.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`

- [x] **Step 1: Write failing shared-fixture parser tests**

```ts
const fixture = JSON.parse(
  readFileSync(
    new URL("../../../../../testdata/protocol/asset_refs_v1.json", import.meta.url),
    "utf8",
  ),
) as AssetRefFixture;

it("matches the Rust canonical fixture corpus", () => {
  for (const testCase of fixture.valid) {
    const parsed = parseAssetRef(testCase.raw);
    expect(parsed?.name).toBe(testCase.name);
    expect(formatAssetRef(parsed!)).toBe(testCase.raw);
  }
  for (const raw of fixture.invalid) expect(parseAssetRef(raw)).toBeNull();
});
```

Add boundary tests for UTF-8 byte length, encoded total length, number overflow,
both-or-neither dimensions, query order, controls/slashes, and uppercase percent
escapes produced by the formatter.

- [x] **Step 2: Write failing message parser tests**

```ts
it("parses a canonical asset outside code", () => {
  expect(parseMessageBody(VALID_REF)).toEqual([
    { type: "asset", asset: parseAssetRef(VALID_REF) },
  ]);
});

it("does not render asset syntax inside inline or fenced code", () => {
  expect(parseMessageBody(`\`${VALID_REF}\``)[0].type).toBe("inline-code");
  expect(parseMessageBody(`\`\`\`text\n${VALID_REF}\n\`\`\``)[0].type).toBe("code-block");
});
```

- [x] **Step 3: Run and verify RED**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/lib/asset-ref.test.ts src/lib/message-parser.test.ts
```

Expected: `asset-ref.ts` and the `asset` Fragment variant do not exist.

- [x] **Step 4: Implement TypeScript grammar parity**

```ts
export interface AssetRef {
  version: 1;
  originRuntimeId: string;
  sha256: string;
  name: string;
  mediaType: string;
  size: number;
  width?: number;
  height?: number;
  raw: string;
}

export function parseAssetRef(raw: string): AssetRef | null;
export function formatAssetRef(asset: Omit<AssetRef, "raw">): string;
```

Use `decodeURIComponent`, `TextEncoder` byte counts, explicit safe integer/u32
checks, and RFC 3986 encoding (`encodeURIComponent` plus escaping `!'()*`). Build
the canonical string and require exact equality with input. Add `^` to the
existing inline prefix group and return `{ type: "asset", asset }`; preserve the
code-block first pass and inline-code priority.

- [x] **Step 5: Write failing upload/client URL tests**

```ts
it("uploads repeated file fields in order", async () => {
  fetchMock.mockResolvedValue(jsonResponse({ ok: true, assets: uploaded }));
  await uploadAssets("room", [fileA, fileB]);
  const [, init] = fetchMock.mock.calls[0];
  expect(init?.body).toBeInstanceOf(FormData);
  expect((init!.body as FormData).getAll("file")).toEqual([fileA, fileB]);
});

it("builds a local resolver URL without exposing fleet base URLs", () => {
  expect(assetResolveUrl("room", parseAssetRef(VALID_REF)!)).toBe(
    "http://127.0.0.1:16868/workspaces/room/assets/resolve/" +
      "3c6a295e-744a-41dc-ba60-5c21bb94e5a2/" + HASH +
      "?name=fleet-assets.png",
  );
});
```

- [x] **Step 6: Implement client methods**

```ts
export interface UploadedAsset extends AssetRef { ref: string }

export async function uploadAssets(
  slug: string,
  files: File[],
  signal?: AbortSignal,
): Promise<ApiResponse<{ assets: UploadedAsset[] }>>;

export function assetResolveUrl(
  slug: string,
  asset: AssetRef,
  options: { download?: boolean } = {},
): string;
```

`uploadAssets` returns `runtime_required` immediately in Browser/WASM mode,
validates mirrored limits, appends repeated `file` fields, and uses
`localNetworkFetch`. `assetResolveUrl` includes only local Runtime base URL,
encoded workspace/origin/hash path, sanitized `name`, and optional `download=1`.

- [x] **Step 7: Run and verify GREEN**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/lib/asset-ref.test.ts src/lib/message-parser.test.ts src/lib/client.assets.test.ts
```

Expected: all tests pass.

- [x] **Step 8: Commit frontend protocol/client**

```bash
git add products/gitim/frontend/src/lib/asset-ref.ts \
  products/gitim/frontend/src/lib/asset-ref.test.ts \
  products/gitim/frontend/src/lib/message-parser.ts \
  products/gitim/frontend/src/lib/message-parser.test.ts \
  products/gitim/frontend/src/lib/client.ts \
  products/gitim/frontend/src/lib/client.assets.test.ts
git commit -m "feat(frontend): parse and upload asset references" \
  -m "Test: npm exec vitest -- run src/lib/asset-ref.test.ts src/lib/message-parser.test.ts src/lib/client.assets.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 10: Scope-keyed attachment draft state

**Files:**

- Create: `products/gitim/frontend/src/hooks/use-attachment-draft-store.ts`
- Create: `products/gitim/frontend/src/hooks/use-attachment-draft-store.test.ts`

- [x] **Step 1: Write failing lifecycle and scope tests**

```ts
it("keeps files isolated across workspace and scope keys", () => {
  const a = attachmentDraftKey("github.com/a/room", "general");
  const b = attachmentDraftKey("github.com/a/room", "dm:alice");
  useAttachmentDraftStore.getState().addFiles(a, [png("a.png")]);
  expect(useAttachmentDraftStore.getState().drafts[a].items).toHaveLength(1);
  expect(useAttachmentDraftStore.getState().drafts[b]).toBeUndefined();
});

it("ignores stale operation completion and preserves newer object URLs", () => {
  const key = attachmentDraftKey("ws", "general");
  const first = useAttachmentDraftStore.getState().addFiles(key, [png("old.png")]);
  const op = useAttachmentDraftStore.getState().beginOperation(key);
  useAttachmentDraftStore.getState().resetDraft(key);
  useAttachmentDraftStore.getState().addFiles(key, [png("new.png")]);
  useAttachmentDraftStore.getState().completeSuccess(key, op.generation);
  expect(useAttachmentDraftStore.getState().drafts[key].items[0].file.name).toBe("new.png");
  expect(URL.revokeObjectURL).toHaveBeenCalledWith(first.accepted[0].previewUrl);
  expect(URL.revokeObjectURL).not.toHaveBeenCalledWith("blob:new.png");
});
```

Cover duplicate selection IDs, count/per-file/aggregate/ref-length errors,
removal, uploaded-ref retention, add-new-after-failed-send, scope switching,
operation status, clear-success, explicit disposal, and exactly-once URL revoke.

- [x] **Step 2: Run and verify RED**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/hooks/use-attachment-draft-store.test.ts
```

Expected: store module is missing.

- [x] **Step 3: Implement the state machine**

```ts
export type AttachmentDraftStatus = "idle" | "uploading" | "sending" | "error";

export interface PendingAttachment {
  id: string;
  file: File;
  previewUrl?: string;
  uploaded?: UploadedAsset;
}

export interface AttachmentDraft {
  generation: number;
  status: AttachmentDraftStatus;
  items: PendingAttachment[];
  error?: string;
}
```

Actions are `addFiles`, `removeItem`, `beginOperation`, `markUploaded`,
`markSending`, `failOperation`, `completeSuccess`, `resetDraft`, and `disposeAll`.
Every async completion requires matching key+generation. Mutations while status
is uploading/sending are rejected. Only approved raster browser types create
preview URLs; full page unload relies on browser cleanup, while explicit reset
and test disposal revoke URLs.

- [x] **Step 4: Run and verify GREEN**

Run: `npm exec vitest -- run src/hooks/use-attachment-draft-store.test.ts`

Expected: all tests pass.

- [x] **Step 5: Commit draft state**

```bash
git add products/gitim/frontend/src/hooks/use-attachment-draft-store.ts \
  products/gitim/frontend/src/hooks/use-attachment-draft-store.test.ts
git commit -m "feat(frontend): track attachment drafts by scope" \
  -m "Test: npm exec vitest -- run src/hooks/use-attachment-draft-store.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 11: Composer paste, picker, upload, and retry flow

**Files:**

- Create: `products/gitim/frontend/src/components/chat/attachment-draft-strip.tsx`
- Modify: `products/gitim/frontend/src/components/chat/input-area.tsx`
- Modify: `products/gitim/frontend/src/components/chat/input-area.test.tsx`
- Modify: `products/gitim/frontend/src/components/chat/chat-layout.tsx`
- Modify: `products/gitim/frontend/src/components/cards/card-detail.tsx`

- [x] **Step 1: Write failing paste/picker/attachment-only tests**

```tsx
it("pastes an image, previews it, and sends an attachment-only message", async () => {
  renderInput({ workspaceSlug: "room", onSend });
  firePaste(textarea(), [png("shot.png")]);
  expect(screen.getByText("shot.png")).toBeTruthy();
  await clickSend();
  expect(uploadAssets).toHaveBeenCalledWith("room", [expect.objectContaining({ name: "shot.png" })]);
  expect(onSend).toHaveBeenCalledWith(`${VALID_REF}`, 0);
});

it("uses uploaded refs on send retry without another upload", async () => {
  onSend.mockResolvedValueOnce({ ok: false, error: "sync failed" });
  onSend.mockResolvedValueOnce({ ok: true });
  renderInput({ workspaceSlug: "room", onSend });
  selectFiles([png("shot.png")]);
  await clickSend();
  await clickSend();
  expect(uploadAssets).toHaveBeenCalledTimes(1);
  expect(onSend).toHaveBeenCalledTimes(2);
});
```

Add tests for paste text-only behavior, paste with file not inserting a binary
placeholder, picker reset so the same file can be selected again, remove, valid
files surviving one invalid file, 10/11 files, mobile button, recipient preview
for attachment-only channel/DM/card messages, reply line preservation, scope
switch during upload, stale completion, Browser mode disabled attachment action,
and URL cleanup after success.

- [x] **Step 2: Run and verify RED**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/components/chat/input-area.test.tsx
```

Expected: no attachment controls or upload call exist.

- [x] **Step 3: Implement visual strip and controls**

`AttachmentDraftStrip` renders compact rounded cards using existing design tokens:
image object-URL thumbnail or file icon, sanitized name, formatted size, per-item
remove button, one inline validation error, and one Uploading/Sending state. No
new colors, fonts, gallery, or animation. Paperclip button and hidden
`<input type="file" multiple>` sit inside the existing input shell and remain
keyboard/aria labelled.

- [x] **Step 4: Implement generation-safe send sequence**

Add required `workspaceSlug: string | null` to InputArea. Effective sendability
is trimmed text or at least one item. Effective routing body uses the text or a
non-empty attachment sentinel so attachment-only messages preview the same owner,
DM members, reply chain, or card roles the final ref body will route.

On send:

1. Capture workspace key, scope key, reply, `onSend`, and operation generation.
2. Upload only items without `uploaded`; store returned refs under that generation.
3. Compose trimmed optional text plus one ref per item in selection order.
4. Mark sending and call the captured existing `onSend`.
5. On failure retain files+refs and set the old draft error.
6. On success clear/revoke only the captured generation and remove its text
   localStorage; touch React text/reply state only when the active scope still
   matches.

- [x] **Step 5: Pass workspace slug from both composer call sites**

`chat-layout.tsx` passes active Runtime workspace slug; `card-detail.tsx` passes
the same slug alongside its existing workspace identity key. Browser/WASM mode
still renders InputArea for text but hides/disables file selection.

- [x] **Step 6: Run and verify GREEN**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/components/chat/input-area.test.tsx src/hooks/use-attachment-draft-store.test.ts
```

Expected: all composer and draft tests pass.

- [x] **Step 7: Commit composer interactions**

```bash
git add products/gitim/frontend/src/components/chat/attachment-draft-strip.tsx \
  products/gitim/frontend/src/components/chat/input-area.tsx \
  products/gitim/frontend/src/components/chat/input-area.test.tsx \
  products/gitim/frontend/src/components/chat/chat-layout.tsx \
  products/gitim/frontend/src/components/cards/card-detail.tsx
git commit -m "feat(frontend): paste and send attachments" \
  -m "Test: npm exec vitest -- run src/components/chat/input-area.test.tsx src/hooks/use-attachment-draft-store.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 12: Inline image and file-card renderer

**Files:**

- Create: `products/gitim/frontend/src/components/chat/asset-fragment.tsx`
- Modify: `products/gitim/frontend/src/components/chat/message-body.tsx`
- Modify: `products/gitim/frontend/src/components/chat/message-body.test.tsx`

- [x] **Step 1: Write failing renderer-state tests**

```tsx
it("renders verified raster metadata as a lazy CORS image", () => {
  render(<MessageBody body={PNG_REF} />);
  const image = screen.getByRole("img", { name: "fleet-assets.png" });
  expect(image.getAttribute("crossorigin")).toBe("anonymous");
  expect(image.getAttribute("loading")).toBe("lazy");
  expect(image.getAttribute("src")).toContain("/assets/resolve/");
});

it("shows metadata-only Runtime-required state in browser mode", () => {
  connectionStore.setState({ mode: "local" });
  render(<MessageBody body={PDF_REF} />);
  expect(screen.getByText("Runtime required")).toBeTruthy();
  expect(screen.getByRole("button", { name: "Download" })).toBeDisabled();
});
```

Add tests for file cards, formatted size/type/origin hint, reserved aspect ratio,
loading, image `onError` unavailable card, Retry recreation, forced download URL,
click propagation, Unicode names, invalid refs as text, SVG/HTML as file cards,
and existing card/message/mention render regressions.

- [x] **Step 2: Run and verify RED**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/components/chat/message-body.test.tsx
```

Expected: canonical refs render as plain text.

- [x] **Step 3: Implement AssetFragment**

The component reads connection mode and active workspace slug from existing
stores. Browser/WASM mode renders a metadata card with a disabled action.
Runtime mode builds only the local resolver URL. Approved raster MIME values use
`<img crossOrigin="anonymous" loading="lazy">`, bounded `max-w-full`, rounded
border, and optional `aspect-ratio`; `onError` swaps to a stable unavailable card
with Retry. Other MIME values use a compact file card and user-activated `<a>`
download/open navigation. All actions stop message click/double-click propagation.

- [x] **Step 4: Dispatch the new fragment**

Add `case "asset": return <AssetFragment key={index} asset={fragment.asset} />;`
to the existing FragmentRenderer. Do not alter existing switch branches or wrap
the whole message in a new block layout; AssetFragment may render a block-level
`span` so mixed text and refs retain whitespace semantics.

- [x] **Step 5: Run and verify GREEN**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/components/chat/message-body.test.tsx src/lib/message-parser.test.ts
```

Expected: asset and existing reference tests pass.

- [x] **Step 6: Commit renderer**

```bash
git add products/gitim/frontend/src/components/chat/asset-fragment.tsx \
  products/gitim/frontend/src/components/chat/message-body.tsx \
  products/gitim/frontend/src/components/chat/message-body.test.tsx
git commit -m "feat(frontend): render image and file attachments" \
  -m "Test: npm exec vitest -- run src/components/chat/message-body.test.tsx src/lib/message-parser.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 13: WASM package and responsive interaction coverage

**Files:**

- Modify: `crates/gitim-wasm/pkg/`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`
- Modify: `products/gitim/frontend/e2e/mobile-layout.spec.ts`

- [x] **Step 1: Add failing browser/WASM metadata regression**

In daemon-web handler tests, parse a thread containing `VALID_REF` and assert the
serialized entry retains body and additive `links[].kind.kind === "asset"`; no
asset byte route or upload method is added to the browser backend.

- [x] **Step 2: Add failing mobile Playwright interaction**

Mock Runtime upload and resolve routes in `mobile-layout.spec.ts`. At mobile
viewport, open the file picker through the paperclip, upload a fixture PNG,
assert the preview/remove/send controls remain inside the composer width, send,
then assert the rendered image is bounded by the message column with no
horizontal page overflow:

```ts
await expect(page.locator("[data-attachment-draft-strip]")).toBeVisible();
await expect(page.locator("[data-asset-image]")).toHaveCSS("max-width", "100%");
expect(await page.evaluate(() => document.documentElement.scrollWidth))
  .toBeLessThanOrEqual(await page.evaluate(() => document.documentElement.clientWidth));
```

- [x] **Step 3: Run targeted tests and verify RED**

Run:

```bash
cd products/gitim/frontend
npm exec vitest -- run src/daemon-web/handlers.test.ts
npm exec -- playwright test e2e/mobile-layout.spec.ts --grep "attachment"
```

Expected: old WASM package has no asset link and mobile UI is absent.

- [x] **Step 4: Rebuild checked-in WASM package**

Run:

```bash
cd products/gitim/frontend
npm run build:wasm
```

Expected: wasm-pack succeeds on the stable toolchain and updates
`crates/gitim-wasm/pkg` deterministically.

- [x] **Step 5: Run WASM and mobile tests and verify GREEN**

Run:

```bash
cargo test -p gitim-wasm --locked
cd products/gitim/frontend
npm exec vitest -- run src/daemon-web/handlers.test.ts
npm exec -- playwright test e2e/mobile-layout.spec.ts --grep "attachment"
```

Expected: all tests pass; Browser/WASM remains metadata-only.

- [x] **Step 6: Commit generated package and responsive coverage**

```bash
git add crates/gitim-wasm/pkg products/gitim/frontend/src/daemon-web/handlers.test.ts \
  products/gitim/frontend/e2e/mobile-layout.spec.ts
git commit -m "feat(wasm): expose asset link metadata" \
  -m "Test: npm run build:wasm && cargo test -p gitim-wasm --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 14: Automated integration verification

**Files:**

- Modify only files required by failures proven in this task.

- [x] **Step 1: Format and run the narrow suites together**

Run:

```bash
cargo fmt --all -- --check
cargo test -p gitim-core --locked
cargo test -p gitim-runtime --features test-support --test assets_store --locked
cargo test -p gitim-runtime --test assets_http --locked
cargo test -p gitim-runtime --test assets_fleet --locked
cargo test -p gitim-runtime --test cli_asset --locked
cargo test -p gitim-runtime --test fleet_http --locked
cargo test -p gitim-runtime --test config_schema --locked
cargo test -p gitim-runtime --test coordinator_prompt --locked
cargo test -p gitim-agent-provider --locked
```

Expected: every command exits 0.

- [x] **Step 2: Run complete frontend verification**

Run:

```bash
cd products/gitim/frontend
npm test
npm run lint
npm run build
npm run test:e2e
```

Expected: Vitest, ESLint, TypeScript/Vite, and configured Playwright specs exit 0.

- [x] **Step 3: Run workspace clippy and full Rust suite**

Run:

```bash
cargo clippy --workspace --all-targets --no-deps --locked
cargo test --workspace --locked
```

Expected: clippy has no denied lint and the full shared-protocol workspace suite
passes. Ignored real-provider tests remain ignored by their existing annotations.

- [x] **Step 4: Audit generated and tracked content**

Run:

```bash
git diff --check
git status --short
rg -n "TODO|TBD|implement later|unimplemented!|todo!" \
  crates/gitim-core/src/types/asset.rs crates/gitim-runtime/src/assets \
  products/gitim/frontend/src/lib/asset-ref.ts \
  products/gitim/frontend/src/components/chat/asset-fragment.tsx
```

Expected: diff check passes and the feature files contain no placeholder or
panic-based implementation.

- [x] **Step 5: Commit only verified integration fixes**

If this task required fixes, stage their exact paths and create one conventional
commit with every command rerun in its `Test:` footer. If no fixes were required,
do not create an empty commit.

### Task 15: MacBook/Mac mini and Kimi WebBridge live E2E

**Files:**

- Create: `docs/plans/file-attachments/03-e2e-evidence.md`
- Create temporary test files only below each workspace's `.gitim-runtime/` or
  system temp directory; never below a Git clone.

- [x] **Step 1: Snapshot and record the live topology**

Record without mutating:

```bash
gitim-runtime status
gitim-runtime runtime-id
gitim-runtime workspaces
gitim-runtime fleet list
gitim-runtime fleet status
ssh <mac-mini-target> 'gitim-runtime status && gitim-runtime runtime-id && gitim-runtime workspaces'
```

Write Runtime versions/IDs, ports, `room` slugs, normalized
`github.com/flame4/room` mappings, original process PIDs, and binary paths into
the evidence document. Confirm the two Runtime IDs differ.

- [x] **Step 2: Build and launch feature binaries on both nodes**

Build the required stable binaries in this worktree. Inspect remote architecture
with `uname -m`; copy only compatible artifacts to a timestamped system-temp
directory. Stop the old Runtime gracefully, start the feature Runtime on its
existing port with logs redirected to a temp path, and wait for `/health` to
report the feature version and expected Runtime ID. Preserve original binary
paths/PIDs so the environment can be restored after evidence collection.

- [x] **Step 3: Create the remote origin and text-only Git message**

Use an existing deterministic repository PNG fixture copied to remote temp. On
Mac mini:

```bash
<feature-runtime> asset put --workspace room --file <remote-temp>/fixture.png
gitim send room '<returned canonical ref>'
sha256sum <remote-temp>/fixture.png
```

Record the canonical ref, source SHA-256, object/sidecar paths, and the resulting
thread commit. Wait until MacBook Git sync sees the exact ref.

- [x] **Step 4: Use Kimi WebBridge for real browser resolution**

Start WebBridge if needed with `~/.kimi-webbridge/bin/kimi-webbridge start`.
Use one session for every browser command:

```json
{
  "session": "gitim-file-attachments-e2e",
  "action": "navigate",
  "args": {
    "url": "http://localhost:5173",
    "newTab": true,
    "group_title": "GitIM 附件双节点 E2E"
  }
}
```

Start network capture, navigate to `room`, snapshot the accessibility tree,
locate the Mac mini message, and verify the image renders. Record the resolver
GET, response headers, screenshot path, MacBook replica object, and matching
SHA-256. Browser DOM must never contain the Fleet peer base URL.

- [x] **Step 5: Prove origin-offline replica behavior**

Stop the Mac mini feature Runtime without removing the Fleet tunnel config.
Reload/recreate the image element through Kimi WebBridge and verify MacBook
returns 200 from its local replica with no peer GET. Record Fleet unavailable
status, network detail, screenshot, and local object hash. Restart Mac mini after
the assertion.

- [x] **Step 6: Prove actual paste and picker interactions**

For paste, use WebBridge `evaluate` only to create a browser `File`, attach it to
a synthetic clipboard event, and dispatch `paste` on the real textarea; React
must handle the normal event path. Use WebBridge `upload` on the hidden file input
for the separate picker case. Snapshot after preview, remove/re-add one item,
send optional text plus attachment, and verify the resulting message/image.
Capture POST `/assets`, existing `/im/send`, GET resolver, desktop screenshot,
and a mobile-width screenshot.

- [x] **Step 7: Prove Agent CLI and Git exclusion**

Run `asset get` for the Mac mini and MacBook refs into temp destinations and
compare source/destination SHA-256. Then audit:

```bash
git -C <human-clone> ls-files
git -C <human-clone> show --stat --oneline <attachment-message-commit>
git -C <human-clone> ls-tree -r --name-only <attachment-message-commit>
rg -n '<\^v1/' <human-clone>/channels <human-clone>/dm
```

Expected: thread text contains refs; no `assets/`, object, sidecar, temp, or PNG
path is tracked or present in the commit tree.

- [x] **Step 8: Restore processes and finalize evidence**

Leave both nodes running a known healthy Runtime version and confirm Fleet status
reconnects. Stop network capture. Keep the WebBridge tab group open for user
inspection. Complete the evidence document with every command/result, hashes,
screenshots, known environment mutations, and restoration result.

- [x] **Step 9: Commit E2E evidence**

```bash
git add docs/plans/file-attachments/03-e2e-evidence.md
git commit -m "test(e2e): verify fleet attachment round trip" \
  -m "Test: MacBook + Mac mini room workspace + Kimi WebBridge live E2E" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 16: Two-round review, requirement audit, and integration-ready gate

**Files:**

- Modify only files required by verified review findings.
- Update: `docs/plans/file-attachments/01-engineering-review.md`
- Update: `docs/plans/file-attachments/03-e2e-evidence.md`

- [x] **Step 1: Run first independent code review**

Use `superpowers:requesting-code-review` over the complete branch diff. Review
against every invariant/success criterion in `00-requirements.md`, not only style.
Separately run the Codex adversarial challenge in read-only mode. Classify every
finding by severity and confidence; reproduce before changing code.

- [x] **Step 2: Fix every confirmed P0/P1/P2 and add regression tests**

Follow `superpowers:receiving-code-review`: verify technically, write the failing
regression first, make the smallest coherent fix, rerun the owning scoped suite,
and record rejected false positives only in transient review notes, not project
artifacts.

- [ ] **Step 3: Run a fresh second review**

Review the post-fix diff with fresh context. The gate requires zero unresolved
P0/P1, zero known data-loss/security gap, and no P2 without an explicit current
design rationale. Repeat regression repair if needed.

- [x] **Step 4: Rerun completion verification from clean current state**

Use `superpowers:verification-before-completion` and rerun:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked
cargo test --workspace --locked
cd products/gitim/frontend
npm test
npm run lint
npm run build
npm run build:wasm
npm run test:e2e
cd ../../..
git diff --check
```

Then re-check the live E2E evidence is from the final binaries or rerun affected
steps if review fixes changed Runtime/Fleet/frontend behavior.

- [x] **Step 5: Requirement-by-requirement completion audit**

Create a table in `03-e2e-evidence.md` mapping every Included item, invariant,
required command, and success criterion to a current file/test/runtime evidence
location. Any missing, indirect, stale, or scope-mismatched evidence keeps the
Goal active and returns execution to the owning task.

- [ ] **Step 6: Update review report and declare integration-ready**

Append the two code-review runs and final verdict to the existing final GSTACK
report section, keeping it last. Confirm `git status`, branch name, ahead count,
and commit log. Do not merge, push, or open a PR. Hand the user the branch,
verification evidence, live E2E result, and available finishing options.

---

## Plan Self-Review Checklist

- [x] Every Included requirement maps to Tasks 1–15.
- [x] Every architectural invariant has a unit/integration/E2E assertion.
- [x] Every new HTTP/CLI/frontend error is recoverable and user-visible.
- [x] Shared protocol changes regenerate and test the checked-in WASM package.
- [x] Runtime uploads/downloads are streaming and resource bounded.
- [x] Local and remote corruption cannot become a served or dedupe-winning object.
- [x] Workspace path reuse cannot expose an old asset namespace.
- [x] No task adds automatic deletion, cloud storage, background prefetch, or
  browser/WASM byte storage.
- [ ] Final verification includes full Rust, frontend, WASM, Playwright, two
  independent reviews, and live two-node browser/Agent/Git audits.
