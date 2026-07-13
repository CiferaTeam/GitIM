# File Attachments v1 — Requirements and Design

Status: APPROVED

## Problem

GitIM messages are durable plain text synchronized by Git. Ordinary IM usage also
needs images and files, but binary payloads would make Git history grow without
bound and would degrade clone, fetch, conflict resolution, mobile storage, and
history rotation.

File Attachments v1 adds a node-local binary data plane while keeping the message
protocol text-only. Git stores a typed, content-addressed asset reference. GitIM
Runtime stores and transfers the referenced bytes. A Runtime-connected frontend
renders local assets directly and resolves remote assets through the existing
Fleet connection for the same workspace identity.

The selected architecture is a content-addressed Fleet Asset Resolver.

## Product Outcome

A human can paste an image or choose files in any Runtime-backed message composer,
preview the pending files, send them with optional text, and later view or download
the attachments. An Agent can publish a local file as an asset reference and fetch
an attachment reference to a local path. Both use the same protocol and object
store.

The resulting `.thread` file contains only text references and display metadata.
Binary content never enters the Git working tree, index, commits, bundles, or
history snapshots.

## Scope

### Included

- Runtime-backed channel, DM, card-thread, and thread-reply composers.
- Image paste and multi-file selection in the React frontend.
- Pending attachment previews scoped to the in-memory draft.
- Inline rendering for verified PNG, JPEG, GIF, WebP, and AVIF assets.
- Download cards for every other file type.
- A typed asset reference in `gitim-core` and the frontend parser.
- A workspace-local, SHA-256-addressed Runtime object store.
- Local reads, Fleet proxy reads, integrity verification, and persistent local
  replicas.
- Runtime CLI commands for Agent upload and download.
- Safe metadata-only rendering in Browser/WASM mode.
- Desktop and mobile layouts shown in [wireframe.html](wireframe.html).

### NOT in scope

- Browser/WASM binary storage, upload, and Fleet transfer.
- Automatic garbage collection or attachment deletion.
- Replication quorum, inventory advertisement, and background repair.
- Cloud object-store backends.
- Resumable or chunked uploads.
- Inline SVG, PDF, audio, or video players.
- Malware scanning and content moderation.

The future mobile-shell path through an automatically selected Tailscale Gateway
Runtime is captured in
[Mobile Gateway Discovery](../mobile-gateway-discovery/00-design.md).

## Architectural Invariants

1. Binary bytes are never written below the human or Agent Git clone.
2. SHA-256 identifies content. A filesystem path never appears in the protocol.
3. `origin_runtime_id` is a lookup hint. Any node holding a verified object with
   the same SHA-256 may serve it.
4. The origin object and every fetched replica remain persistent in v1. Runtime
   does not delete an asset based on age or access time.
5. Assets are isolated by workspace. A hash stored for one workspace does not
   authorize reads from another workspace.
6. Cross-node workspace resolution uses the existing Git remote
   `workspace_identity`; local and remote workspace slugs may differ.
7. The browser only talks to its connected Runtime. Runtime performs Fleet
   routing and never exposes a peer `base_url` to message HTML.
8. All received bytes are hashed before becoming visible in the local object
   store or being returned to a caller.
9. Asset metadata in a message is untrusted display data. Server-side object
   metadata and byte inspection determine response headers and inline safety.
10. Older clients that do not recognize the new sigil continue to display the
    reference as plain message text.

Persistence means "kept until the workspace directory is explicitly removed,"
not disaster-proof storage. Before another node resolves an object, the origin
is the only copy; loss of that node or disk can leave the reference permanently
unavailable. `.gitim-runtime/` is intentionally excluded from Time Machine, so
v1 assets are best-effort node-local data rather than a backup system.

## Protocol

### Canonical reference

The canonical v1 reference is one line with no literal whitespace:

```text
<^v1/{origin_runtime_id}/sha256:{sha256}?name={name}&type={media_type}&size={size}[&width={width}&height={height}]>
```

Example:

```text
<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88?name=fleet-assets.png&type=image%2Fpng&size=184203&width=1600&height=900>
```

`^` is the asset-link sigil. Existing GitIM sigils remain unchanged:
`@` mention, `#` channel/message/card, `~` user profile, and `!` soft link.

### Fields

| Field | Rule |
|---|---|
| `v1` | Exact supported schema version. |
| `origin_runtime_id` | Lowercase dashed UUID. |
| `sha256` | Exactly 64 lowercase hexadecimal characters. |
| `name` | UTF-8 filename, RFC 3986 percent-encoded, 1–255 decoded bytes, subject to the total ref limit. |
| `media_type` | Lowercase MIME type, percent-encoded, at most 127 decoded bytes. |
| `size` | Unsigned decimal byte length, at most the v1 per-file limit. |
| `width`, `height` | Optional positive decimal image dimensions; both are present or both absent. |

The query order is canonical. Unknown keys, duplicate keys, fragments, user-info,
non-canonical UUIDs, uppercase hashes, decoded control characters, `/`, `\\`, and
references longer than 1024 bytes are rejected as typed assets and remain plain
text.

Upload validates the fully formatted reference before persisting the batch. A
filename that fits the decoded 255-byte limit but whose encoded form would push
the complete reference above 1024 bytes is rejected with `invalid_asset`;
Runtime never returns a reference its own parser rejects. The frontend performs
the same preflight check.

The filename is a basename for display and `Content-Disposition`; it is never
joined into an object path. Uploaders take the basename, remove control
characters, and fall back to `attachment` when no valid name remains.

### Core types

`gitim-core` owns parsing, validation, and canonical formatting:

```rust
pub struct AssetRef {
    pub version: u8,
    pub origin_runtime_id: String,
    pub sha256: String,
    pub name: String,
    pub media_type: String,
    pub size: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
}
```

`LinkKind` gains an `Asset { asset: AssetRef }` variant. `AssetRef::parse` and
`Display` form the protocol boundary. Runtime, CLI, WASM bindings, and frontend
fixtures consume the same canonical shape.

### Message composition

After upload succeeds, the composer appends one canonical reference per line:

```text
Optional human text.
<^v1/...>
<^v1/...>
```

An attachment-only message consists only of references and is a valid non-empty
message. Mention extraction, recipient routing, reply chains, line numbering, and
Git commit behavior continue to operate on the final text body.

## Runtime Asset Store

### Location

Each workspace owns one store outside every Git clone:

```text
<workspace>/.gitim-runtime/assets/v1/
├── store.json
├── objects/sha256/ab/<full-sha256>
├── metadata/sha256/ab/<full-sha256>.json
├── locks/sha256/ab/<full-sha256>.lock
└── tmp/
```

The first two hash characters shard directories. Object paths are derived only
from validated lowercase hashes.

`store.json` binds the namespace to the workspace. GitHub workspaces use their
normalized remote identity; local workspaces use the workspace config creation
identity. Every read and write verifies this binding. Reusing a path for a
different workspace moves the old `assets/v1` tree to a timestamped
`orphaned-assets/` directory before creating an empty bound store, preserving
bytes without exposing them to the new workspace.

On Unix, asset directories use mode `0700` and object, metadata, and temporary
files use mode `0600`. `DELETE /workspaces/{slug}` unregisters the workspace and
preserves its on-disk directory, including assets. Local assets disappear only
when the workspace directory is explicitly removed. Failed workspace creation
may remove its partial `.gitim-runtime/` tree before any message can reference an
asset.

### Metadata sidecar

The sidecar is local operational state, not protocol state:

```json
{
  "schema_version": 1,
  "sha256": "8f2c...",
  "size": 184203,
  "media_type": "image/png",
  "width": 1600,
  "height": 900,
  "object_modified_ns": 1783692000000000000,
  "stored_at": "2026-07-10T14:00:00Z",
  "source": {
    "kind": "local_upload"
  }
}
```

A Fleet-fetched object records `kind: "fleet_replica"` and the requested origin
Runtime ID. Filename is not stored in the sidecar because multiple messages may
refer to identical bytes under different names.

The object is authoritative. A missing or invalid sidecar is reconstructed by
re-hashing and inspecting the object. A sidecar without its object is ignored
and replaced on the next successful put or fetch. A hot read trusts the sidecar
only when object length and modification timestamp still match. Dedupe hits
always re-hash the existing object before discarding incoming known-good bytes;
an invalid existing object is quarantined and replaced atomically.

### Write path

1. Stream request chunks into a unique file in `tmp/`.
2. Track per-file and per-request byte limits while streaming.
3. Calculate SHA-256 incrementally.
4. Inspect magic bytes and image headers without decoding the full image.
5. Flush and close the temporary file.
6. Persist without replacing an existing object. An existing hash is a dedupe
   hit.
7. Atomically write or repair the sidecar.
8. Return a canonical reference using the current Runtime ID.

Temporary files are removed on validation, transport, hashing, or persistence
failure. A crash may leave files in `tmp/`; Runtime removes only GitIM-owned
temporary files older than 24 hours at startup so a second process cannot erase
an upload that is still in progress.

Every GET resolution takes a workspace-and-hash filesystem lock before a remote
fetch, rechecks the local store after acquiring it, and releases it only after
the verified object and sidecar are durable. This provides cross-task and
cross-process singleflight without an unbounded in-memory lock map. Concurrent
identical uploads still use atomic no-clobber persistence and converge on one
object.

A valid sidecar plus matching object length and modification timestamp is trusted
on the hot local-read path. Reconstructing a missing or invalid sidecar re-hashes
the object before it is accepted. A local object that does not match its path
hash is quarantined and resolution proceeds to Fleet; corrupt local bytes are
never served.

### Storage pressure controls

- Default persistent quota is 20 GiB per workspace, configurable with
  `GITIM_ASSET_WORKSPACE_QUOTA_BYTES`.
- Runtime preserves at least the greater of 2 GiB or 5% of filesystem capacity,
  configurable with `GITIM_ASSET_MIN_FREE_BYTES`.
- The object store is scanned once when a workspace is recovered; successful
  no-clobber writes update in-memory byte and object counters.
- At most two multipart upload requests run concurrently per Runtime. Peer
  transfers keep their separate four-transfer cap.
- Free-space checks run before accepting a request and while streaming chunks,
  so temporary files cannot consume the reserve before persistence.
- Origin objects and replicas both count toward quota. Runtime never evicts an
  existing object automatically; quota failure leaves every existing ref intact.

### v1 limits

| Limit | Value |
|---|---:|
| Attachments per upload request/message | 10 |
| Bytes per attachment | 50 MiB |
| Aggregate bytes per upload request | 200 MiB |
| Decoded filename | 255 bytes |
| Canonical asset reference | 1024 bytes |

Limits are enforced in Runtime and mirrored in the frontend for immediate
feedback. Runtime remains authoritative.

### Content inspection

Runtime derives the stored media type from magic bytes. Unknown content is
`application/octet-stream`. The browser-provided type and filename extension do
not make content inline-safe.

Only verified PNG, JPEG, GIF, WebP, and AVIF objects receive an inline image
response. SVG and HTML are always attachment downloads. Image dimensions are read
from bounded headers and are display hints; the frontend clamps rendered size to
its container.

## Runtime HTTP API

All routes are workspace scoped.

### Upload

```http
POST /workspaces/{slug}/assets
Content-Type: multipart/form-data
```

The request contains repeated `file` fields. Runtime validates every temporary
file before persisting the batch.

The asset router disables Axum's small default body limit only for this route and
applies an explicit request-body ceiling equal to the aggregate limit plus bounded
multipart overhead. Other Runtime routes keep their existing limits.

Success:

```json
{
  "ok": true,
  "assets": [
    {
      "ref": "<^v1/...>",
      "sha256": "8f2c...",
      "name": "fleet-assets.png",
      "media_type": "image/png",
      "size": 184203,
      "width": 1600,
      "height": 900
    }
  ]
}
```

The frontend calls this endpoint when the user presses Send, then appends the
returned references and invokes the existing channel, DM, or card-message send
method. This preserves existing author identity and avoids separate multipart
send implementations for each message target.

If the message send fails after upload, persisted assets remain valid. The draft
keeps the returned references so a retry does not upload again. Removing such a
failed draft may leave an unreferenced origin object; v1 favors durable bytes over
unsafe automatic deletion.

### Resolve for a local caller

```http
GET|HEAD /workspaces/{slug}/assets/resolve/{origin_runtime_id}/{sha256}
```

Optional query fields:

- `name`: sanitized download filename copied from the parsed message ref.
- `download=1`: force `Content-Disposition: attachment`.

Resolution order:

1. Serve a valid local object with the requested hash, regardless of origin.
2. Look up the Fleet entry whose verified `runtime_id` matches the origin and
   whose workspace mapping targets the current local workspace identity.
3. Fetch that peer's node-local object endpoint.
4. If the origin is unavailable or lacks the object, try all other configured
   Fleet nodes mapped to the same workspace identity. This is time- and
   concurrency-bounded lookup, not inventory or replication coordination.
5. Stream the peer response to a temporary file, enforce limits, verify SHA-256,
   persist it as a replica, then serve the local copy.

Runtime never returns unverified peer bytes to the caller.

Replica creation is pull-through and demand-driven. Runtime does not prefetch
assets merely because `/im/poll` observed their references: that endpoint is
driven by a mounted frontend and reports workspace-wide changes, including
channels the human may never open. A replica is created only when a user or
Agent resolves or downloads the asset. Until that first resolution, the origin
node remains the sole durable copy.

Fallback and transfer bounds are explicit:

- the origin is always attempted first;
- every eligible fallback alias is sorted before probing so logs and tests are
  reproducible and a valid fourth-or-later replica cannot be hidden by a count
  cap;
- fallback availability HEAD probes run concurrently, but full object GETs are
  attempted one at a time; HEAD concurrency is capped at eight so one resolution
  never creates unbounded sockets or downloads multiple 50 MiB copies;
- peer connect, response-header, chunk-idle, candidate-transfer, and whole
  resolution budgets are 5, 10, 15, 90, and 120 seconds respectively;
- one Runtime permits at most four concurrent peer object transfers, while
  per-hash singleflight collapses duplicate callers.

If one candidate returns corrupt or oversized bytes, Runtime discards the
temporary file and continues to another verified candidate while budget remains.
If no candidate succeeds, hash mismatch takes precedence over unavailable and
missing errors because it is the most actionable integrity failure.

`HEAD` does not download or persist a missing remote object. It performs a local
metadata lookup or bounded peer `HEAD` probe and reports availability. Only `GET`
creates a replica.

### Node-local object endpoint

```http
GET|HEAD /workspaces/{remote_slug}/assets/objects/{sha256}
```

This endpoint serves only the receiving Runtime's local store. It never performs
Fleet forwarding, so proxy loops are impossible.

### Response behavior

Successful responses include:

- authoritative `Content-Type`;
- `Content-Length`;
- `ETag: "sha256-{hash}"`;
- browser-facing `/assets/resolve` responses use
  `Cache-Control: private, no-store`, including `HEAD`, conditional, range, and
  error responses, so workspace slug reuse cannot expose bytes from a prior
  browser cache entry;
- node-local `/assets/objects` responses use
  `Cache-Control: private, immutable, max-age=31536000` because the URL is
  content-addressed and unavailable to browser contexts;
- `X-Content-Type-Options: nosniff`;
- safe `Content-Disposition`;
- single-range `Accept-Ranges: bytes` support for local objects.

An exact `If-None-Match` for the SHA-256 ETag returns `304`. File
streaming and single-range parsing reuse Tower HTTP's `ServeFile` implementation;
GitIM only supplies verified MIME/disposition/cache headers and the content hash
ETag rather than maintaining a second hand-written range engine.

The first remote resolution downloads and validates the complete object before
serving a requested range. Subsequent range requests use the local replica.

### Error codes

| HTTP | `error_code` | Meaning |
|---:|---|---|
| 400 | `invalid_asset_ref` | Invalid origin/hash/name parameters. |
| 400 | `invalid_asset` | Upload metadata or content is invalid. |
| 413 | `asset_too_large` | Per-file limit exceeded. |
| 413 | `asset_request_too_large` | Aggregate request limit exceeded. |
| 422 | `too_many_assets` | More than ten files. |
| 403 | `asset_origin_forbidden` | Browser origin is not allowed. |
| 404 | `asset_missing` | A reachable origin/replica does not hold the hash. |
| 502 | `asset_hash_mismatch` | Peer bytes do not match the requested hash. |
| 502 | `asset_peer_invalid` | Peer response is malformed or exceeds the object limit. |
| 503 | `asset_origin_unavailable` | No mapped Fleet peer can currently answer. |
| 507 | `asset_quota_exceeded` | Workspace quota or free-space reserve would be crossed. |
| 507 | `asset_store_failed` | Local persistence failed. |

## Fleet Identity and Routing

`FleetNodeEntry.node_id` is a local operator-facing alias such as `mac-mini`; it
is not the remote node's protocol identity. `FleetNodeEntry` gains an optional
`runtime_id` field populated from the remote `/health` response.

```json
{
  "node_id": "mac-mini",
  "runtime_id": "3c6a295e-744a-41dc-ba60-5c21bb94e5a2",
  "base_url": "http://127.0.0.1:18068"
}
```

Fleet add and tunnel-up fetch `/health`, require the `gitim-runtime` service, and
persist the returned UUID. Two aliases may not register the same Runtime ID.

Legacy entries without `runtime_id` continue to deserialize. Startup recovery
and the first asset resolution probe `/health` and persist a best-effort backfill.
An unreachable legacy node remains usable for existing SSE retry behavior and is
ineligible as a specific asset origin until identity verification succeeds.

Runtime-ID discovery updates both the live Fleet entry and `runtime.json` with an
atomic, locked read-modify-write. All existing Runtime writers are migrated to
the same helper so a legacy backfill cannot overwrite a concurrent workspace or
Fleet edit. If discovery would give two aliases the same Runtime ID, the existing
verified alias remains authoritative, the duplicate is logged, and the legacy
entry remains ineligible for origin matching.

The current workspace mapping remains authoritative:

```text
local workspace slug
  -> workspace_identity (normalized Git remote)
  -> Fleet entry mapping
  -> remote workspace slug
```

No workspace slug appears in the asset reference. `origin_runtime_id` remains a
routing hint rather than asset identity: after a Runtime reinstall, an existing
local hash still wins and the renamed node can still be found as a
workspace-matched fallback.

## Browser-Origin Boundary

Existing Runtime CORS is permissive. File upload increases the impact of a
malicious webpage reaching localhost, so asset routes add a browser-origin guard.

- Browser requests are classified with both `Origin` and `Sec-Fetch-Site`.
  Cross-site requests are accepted only from the GitIM production origin and
  configured development origins.
- CLI and Runtime-to-Runtime requests omit `Origin` and remain accepted; local
  processes and Tailnet/SSH peers are within the existing host trust boundary.
- Allowed Web origins are configurable through `GITIM_WEB_ORIGINS` for self-hosted
  deployments.
- The frontend never injects a peer address into DOM URLs.

Default allowed origins are `https://gitim.io`, `https://www.gitim.io`, and
loopback Vite origins used for development. Preview deployments are not trusted
by wildcard; a specific preview or self-hosted origin must be added explicitly.
`GITIM_WEB_ORIGINS` adds comma-separated exact origins; it does not accept a
wildcard.

The origin guard applies to upload and resolve routes. The node-local object
endpoint rejects browser-context requests entirely; it accepts Runtime-to-Runtime
and CLI traffic without browser fetch headers.

Inline images use a credentialless CORS image request (`crossorigin="anonymous"`)
so the browser sends an `Origin` header and Runtime can enforce the allowlist.
Direct open/download links are accepted only as top-level, user-activated browser
navigations (`Sec-Fetch-Mode: navigate`, `Sec-Fetch-Dest: document`, and
`Sec-Fetch-User: ?1`) or as allowed-origin fetches. Upload never accepts the
navigation exception. Requests carrying browser Fetch Metadata but neither an
allowed origin nor the navigation tuple are rejected. This keeps downloads
streaming instead of materializing a 50 MiB Blob in mobile browser memory.

## Runtime CLI and Agent Contract

### Upload

```text
gitim-runtime asset put --workspace room --file ./report.pdf
```

Output is JSON containing the canonical `ref` and metadata. `--file` is
repeatable up to the request limit. Workspace selection follows existing CLI
rules: explicit `--workspace`, otherwise the only configured workspace.

### Download

```text
gitim-runtime asset get \
  --workspace room \
  --ref '<^v1/...>' \
  --output ./report.pdf
```

The CLI parses the reference with `gitim-core`, calls the local Runtime resolver,
streams to a temporary output file, verifies SHA-256, and atomically persists the
destination. It refuses to overwrite an existing path unless `--force` is set.
Without `--output`, it uses the sanitized reference filename in the current
directory.

The provider system prompt documents this workflow:

1. Publish bytes with `gitim-runtime asset put`.
2. Copy the returned reference into `gitim send` or `gitim dm send`.
3. Fetch a received attachment with `gitim-runtime asset get` before inspecting
   it with local tools.

## Frontend Interaction

### Composer

- A paperclip/add button opens a hidden multi-file input.
- Pasting clipboard files in the textarea adds them to the pending draft and does
  not paste a binary placeholder into text.
- Raster images receive local object-URL thumbnails. Other files receive compact
  type cards.
- Each pending item shows name and formatted size and has a remove control.
- Client-side count and size validation reports errors without discarding the
  text draft or valid pending files.
- Pending files survive channel/DM/card switching during the current page session
  through a draft-keyed in-memory store. A full reload clears them.
- Object URLs are revoked on removal, successful send, and final store cleanup.
- While uploading, send is disabled and the composer shows a single progress
  state. The text and pending files are restored on failure.
- Every draft has a monotonically increasing operation generation. Upload/send
  completion mutates only the captured workspace, scope, and generation; stale
  completion cannot clear a newer draft or revoke object URLs owned by it.

### Send sequence

1. Upload pending files through `uploadAssets`.
2. Store returned references on the pending draft.
3. Compose the final text body.
4. Call the existing `onSend` callback for the current target.
5. Clear text, pending files, references, and local object URLs only after the
   message send succeeds.

Retries reuse returned references and do not re-upload successful objects.

### Message rendering

The frontend parser gains an `asset` fragment matching the core grammar.

- Verified raster types render as bounded inline images with filename and size.
- Clicking an image opens the verified resolver URL in a new tab.
- Other types render a file card with name, media type, size, origin hint, and a
  Download action.
- Loading states reserve image dimensions when available.
- A failed resolver request becomes a stable unavailable card with Retry.
- Invalid references remain selectable plain text.
- Browser/WASM mode renders metadata and a disabled `Runtime required` action.

The interaction follows the existing dark, friendly, compact design system and
the approved [wireframe](wireframe.html). No separate attachment gallery or media
manager is introduced.

## Compatibility

- `.thread` syntax remains text and continuation-line compatible.
- Old parsers ignore the unrecognized `^` link and preserve the body.
- `LinkKind::Asset` is additive in serialized read/poll responses.
- Frontend runtime parsing and daemon-web WASM parsing receive matching fixtures.
- Because `gitim-core` protocol logic changes, `gitim-wasm/pkg/` is rebuilt and
  committed. Browser/WASM storage behavior remains metadata-only.
- Fleet config uses an optional field so existing `runtime.json` files load.
- Existing JSON send, DM send, and card-message endpoints remain unchanged.

## Observability

Runtime logs structured events for upload, dedupe, local hit, origin hit,
fallback-replica hit, unavailable origin, hash mismatch, and store failure. Logs
include workspace slug, hash prefix, byte count, origin Runtime ID, and Fleet
alias; they never include file bytes or a full local source path.

`/runtime/health` adds counters:

- `asset_store_failures`
- `asset_hash_mismatches`
- `asset_fleet_fetch_failures`

Each `workspace_epochs` entry also reports `asset_bytes`, `asset_objects`, and
the effective workspace quota from the recovery-time scan and successful writes;
the hot health path never scans the filesystem.

The existing Fleet status remains the operator view for peer connectivity.

## Verification Strategy

### Core

- Canonical parse/format round trip.
- Every field boundary and rejected non-canonical form.
- Multiple assets among mentions, cards, soft links, and code blocks.
- Old message bodies remain unchanged.
- Serialized `LinkKind::Asset` wire shape.

### Runtime store

- Streaming upload, dedupe, atomic persistence, permissions, and temp cleanup.
- Magic-byte media type and image-dimension extraction.
- Unknown and active content forced to download behavior.
- Per-file, aggregate, filename, and count limits.
- Fully encoded 1024-byte reference boundary.
- Workspace binding mismatch quarantine, quota, free-space reserve, and upload
  concurrency.
- Sidecar reconstruction and object/sidecar partial-state recovery.
- Concurrent identical uploads produce one valid object.
- Same-length external corruption changes metadata, is detected, and never wins
  a dedupe race over known-good incoming bytes.
- A child-process test proves the filesystem singleflight lock; crash-point
  fixtures cover object-only, sidecar-only, and recent/stale temporary states.

### Runtime HTTP and Fleet

- Upload and local GET/HEAD/range responses.
- Browser-origin rejection and allowed-origin success.
- Local hash wins regardless of origin hint.
- Remote workspace-slug mapping by workspace identity.
- Origin fetch, fallback peer fetch, persistent replica, and subsequent offline
  local read.
- A valid replica beyond the third sorted Fleet alias is found; HEAD fanout and
  GET transfer concurrency remain bounded.
- Remote 404, timeout, oversize response, corrupt response, and hash mismatch.
- Legacy Fleet entry Runtime-ID backfill.
- Concurrent child-process Runtime config updates preserve both mutations.

### CLI and Agent prompt

- Asset put/get argument parsing, single-workspace default, multiple-workspace
  error, JSON output, overwrite protection, and hash verification.
- Prompt contract mentions both commands and the send workflow.

### Frontend

- Parser parity for canonical and invalid references.
- Paste, picker, remove, count/size error, scope switching, retry, and object-URL
  cleanup.
- Stale upload/send generations cannot mutate a newer draft after navigation.
- Existing channel, DM, and card sends receive final text with references.
- Inline image, file card, loading, unavailable, retry, and Browser-mode states.
- Desktop and mobile layout regression tests.

### Local two-node E2E

Use the connected MacBook and Mac mini `room` workspace.

1. Build and run the feature Runtime on both nodes.
2. Verify MacBook Runtime ID and Mac mini Runtime ID are distinct.
3. Verify the MacBook Fleet entry `mac-mini` records the Mac mini Runtime ID and
   maps `github.com/flame4/room` to the local `room` workspace.
4. On Mac mini, upload a deterministic PNG with `gitim-runtime asset put` and send
   its returned ref into `room`.
5. Wait for Git sync and open the message in the MacBook WebUI.
6. Verify MacBook Runtime fetches through the existing SSH tunnel, validates the
   hash, stores a replica, and renders the image.
7. Stop the Mac mini Runtime and reload the asset from the MacBook local replica.
   `fleet tunnel down` is not used for this assertion because the existing Fleet
   watcher automatically re-establishes a configured tunnel.
8. Paste and send an image from the MacBook WebUI and verify the composer flow.
9. Fetch both references through the Agent CLI and compare SHA-256 values.
10. Verify `git ls-files`, the relevant commit tree, and `.thread` content contain
    references but no asset object or binary payload.
11. Corrupt a served test object in an isolated fixture and verify the receiving
    Runtime rejects it without persisting or rendering it.

### Required test commands

Implementation verification must select scoped tests during development, then run
the full workspace suite because the change affects shared protocol types:

```text
cargo test -p gitim-core --locked
cargo test -p gitim-runtime --locked
cargo test -p gitim-agent-provider --locked
npm test
npm run lint
npm run build
npm run build:wasm
cargo test --workspace --locked
git diff --check
```

The live two-node E2E is required in addition to automated tests.

## Success Criteria

- A pasted image is sent and rendered without binary data entering Git.
- An arbitrary file is sent and downloaded with safe headers.
- A Mac mini-origin image resolves through the MacBook Fleet tunnel.
- The MacBook continues serving its verified replica while Mac mini is offline.
- Agent put/get round trips preserve SHA-256.
- Invalid paths, metadata, origins, oversize content, and corrupt peer bytes do
  not escape validation or become stored objects.
- Browser/WASM mode preserves readable message history without claiming binary
  availability.
- Existing text-only messages and send paths remain behaviorally unchanged.

## Distribution

The existing release pipeline publishes updated `gitim-runtime`, `gitim`, and
`gitim-daemon` binaries. The existing frontend deployment publishes the renderer
and composer changes. Fleet asset transfer requires a Runtime version containing
the node-local object endpoint on both peers; mixed versions degrade to an
explicit unavailable response and never corrupt message history.

## Next Phase

Engineering review is complete in [01-engineering-review.md](01-engineering-review.md).
Implementation proceeds through a test-first milestone plan covering protocol,
storage and Fleet, CLI, WASM/frontend, automated integration, and the live
MacBook/Mac mini E2E.
