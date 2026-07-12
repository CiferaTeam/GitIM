# File Attachments v1 — Engineering Review

Status: FINAL REVIEW IN PROGRESS

Reviewed: 2026-07-12

Source of truth: [00-requirements.md](00-requirements.md)

## Scope Challenge

This feature necessarily crosses the message protocol, Runtime storage and HTTP,
Fleet discovery, Agent CLI, WASM compatibility, and the React composer/renderer.
The change will exceed eight files, but the breadth is the essential vertical
slice rather than parallel infrastructure. The full v1 scope is retained and is
implemented as independently verified milestones so no reliability feature is
silently deferred.

The selected replication policy is demand-driven pull-through caching. Runtime
does not prefetch workspace-wide `/im/poll` changes; a verified replica is made
only when a human or Agent actually resolves the asset.

## What already exists

| Existing capability | Reuse in this feature |
|---|---|
| `gitim-core::link` typed link extraction | Add the `^` asset kind and canonical parser without a second Rust grammar. |
| `Message.body` plus continuation lines | Store one canonical AssetRef per line with no thread-format change. |
| Runtime workspace-scoped router and `WorkspaceContext.path` | Locate a store outside all Git clones and keep every route workspace-isolated. |
| `UserConfig.runtime_id` and `/health` | Use the stable Runtime UUID as the origin hint instead of Fleet aliases. |
| Fleet workspace identity mappings and SSH tunnel watcher | Route a local workspace to the matching remote slug without exposing peer URLs to the browser. |
| `InputArea` shared by channels, DMs, replies, and card threads | Add one attachment draft/send flow rather than four send implementations. |
| Frontend message parser and `MessageBody` renderer | Add one asset fragment and renderer while preserving code spans and existing link behavior. |
| `gitim-runtime` HTTP CLI wrapper | Add Agent put/get commands with existing port and workspace discovery. |
| `gitim-wasm` link serialization | Rebuild the checked-in package so browser-only history can preserve asset metadata. |
| Existing binary and frontend release workflows | Ship through current artifacts; no new distributable is introduced. |

There is no existing binary object store, integrity-checked Fleet fetch path, or
attachment draft state to reuse.

## Architecture Review

### End-to-end data flow

```text
UPLOAD / SEND

Browser or Agent
    │ multipart stream
    ▼
Runtime asset upload
    ├─ limits + filename validation
    ├─ tmp file + incremental SHA-256
    ├─ magic type + bounded dimensions
    └─ atomic no-clobber object + sidecar
             │
             └── canonical <^v1/runtime-id/sha256:...?...>
                         │
                         ▼
                existing IM send path
                         │
                         ▼
                    text-only Git

RESOLVE / REPLICA

MessageBody or Agent CLI
    │ local Runtime URL only
    ▼
workspace + hash validation
    ├─ valid local object ───────────────────────────────┐
    └─ miss                                              │
         │ per-workspace/hash filesystem singleflight    │
         ▼                                               │
    origin Runtime ID → Fleet mapping → remote slug      │
         │                                               │
         ├─ origin node-local GET                        │
         └─ bounded fallback HEAD → one GET              │
                    │                                    │
                    ▼                                    │
             tmp + size + SHA-256                        │
                    │                                    │
             persistent local replica                    │
                    └────────────────────────────────────┘
                                      │
                                      ▼
                      verified/range-safe local response

Browser/WASM mode: parse metadata → show Runtime-required state; no byte path.
```

### Boundaries and invariants

- `gitim-core` owns AssetRef validation and canonical formatting.
- Runtime owns bytes, authoritative MIME inspection, storage, Fleet transfer,
  security headers, and integrity verification.
- Frontend owns draft files and presentation only; it never receives a peer URL.
- HTTP handlers snapshot workspace/Fleet data under the Runtime mutex and release
  it before filesystem or network awaits.
- Full peer GETs are sequential within one resolution, globally capped at four,
  and collapsed per workspace/hash by a filesystem lock.
- Every eligible workspace-matched peer is discoverable; HEAD fanout is capped
  at eight and whole-resolution time is bounded instead of truncating candidates.
- Runtime config discovery uses atomic locked read-modify-write so identity
  backfill cannot discard concurrent Fleet/workspace edits.
- Each store has a workspace binding manifest, a configurable 20 GiB default
  quota, a free-space reserve, and recovery-time usage accounting.
- Local hot reads require matching sidecar length and modification timestamp;
  recovery and dedupe paths re-hash before trusting existing bytes.
- Browser upload and subresource reads require an allowed Origin. Only a
  top-level user-activated navigation may use the download/open exception.
- Node-local object routes never forward, which makes proxy loops impossible.

### Component layout

Runtime asset code is a dedicated module with store, resolver, and HTTP
submodules. The existing `http.rs` only nests the asset router and exposes shared
state. Fleet identity discovery stays in `fleet`, canonical refs stay in core,
and CLI transport stays in `cli`; binary logic is not duplicated across those
surfaces.

## Code Quality Review

- A shared JSON corpus contains valid canonical refs and invalid boundary cases;
  Rust and Vitest consume the same fixtures to prevent parser drift.
- Limits and protocol constants have one Rust definition and one frontend mirror
  locked by fixture/boundary tests; numeric literals are not scattered across
  components.
- Runtime storage errors use a typed error enum mapped once to stable HTTP error
  codes and CLI exit behavior.
- Store paths are derived only from validated hashes. Display filenames never
  participate in filesystem path construction.
- Sidecar recovery is a store concern, Fleet selection is a resolver concern,
  and response/range construction is an HTTP concern.
- `tower_http::services::ServeFile` supplies the proven single-range file engine;
  GitIM adds only verified headers and SHA-256 ETag behavior.
- Pending attachment state is keyed by workspace and scope outside the mounted
  composer, so navigation during upload updates/restores the correct draft.
- Draft operations carry generations; stale upload/send completion cannot mutate
  a newer draft at the same scope.
- Object URLs are owned by draft items and revoked exactly once on removal,
  success, replacement, or final store cleanup.

## Test Review

```text
CODE PATHS                                               USER FLOWS
[VERIFIED ★★★] Core AssetRef grammar                    [VERIFIED ★★★ →E2E] Paste image + optional text + send
  ├─ canonical parse/format/serde                          ├─ pending preview/remove/scope switch
  ├─ every field boundary and percent encoding             ├─ upload then existing IM send
  └─ invalid ref remains plain text                         └─ rendered no-store image

[VERIFIED ★★★] Runtime upload                           [VERIFIED ★★★ →E2E] Pick arbitrary file + download
  ├─ multipart stream/count/file/aggregate limits           ├─ file card and safe filename
  ├─ tmp cleanup + magic MIME + dimensions                  └─ range/download/hash match
  └─ binding/quota/reserve + atomic dedupe/repair

[VERIFIED ★★★] Runtime resolve/Fleet                    [VERIFIED ★★★ →E2E] Mac mini origin → MacBook replica
  ├─ local GET/HEAD/ETag/range                             ├─ resolve through existing tunnel
  ├─ origin mapping + legacy ID backfill                   ├─ verify stored replica
  ├─ timeout/404/oversize/corrupt/hash mismatch             └─ origin offline, replica still renders
  └─ fallback bound + singleflight + transfer semaphore

[VERIFIED ★★★] Browser security                        [VERIFIED ★★★] Recoverable user errors
  ├─ allowed/rejected Origin and Fetch Metadata             ├─ upload failure keeps draft and refs
  ├─ browser resolve no-store across slug reuse             ├─ unavailable card + Retry
  └─ node-local browser rejection                           └─ stale generations and workspace disposal

[VERIFIED ★★★] Agent CLI                               [VERIFIED ★★★ →E2E] Agent put/ref/send/get
  ├─ workspace selection + streaming multipart              └─ destination SHA-256 equals source
  └─ temp output/hash/overwrite protection

[VERIFIED ★★★] WASM/frontend parser and renderer       [VERIFIED ★★★] Browser/WASM metadata history
  ├─ shared grammar fixtures                                └─ Runtime-required action is disabled
  ├─ code-span/plain-text behavior
  └─ image/file/loading/unavailable/mobile states

COVERAGE: every included requirement and architectural invariant maps to a
unit, integration, browser, or final two-node assertion in 03-e2e-evidence.md.
```

Legend: ★★★ behavior + boundary + error coverage; `→E2E` crosses three or more
components or would be weakened by mocks.

## Production Failure Modes

| Path | Realistic failure | Planned test | Handling | User-visible result |
|---|---|---|---|---|
| Upload stream | Client disconnects after partial body | Multipart abort integration | Remove owned temp files | Draft remains with retry error |
| Upload batch | Later file violates aggregate limit | Batch boundary integration | Persist no refs; clean all temps | Specific size error |
| Storage pressure | Concurrent uploads approach quota/free reserve | Semaphore/quota integration | Reject before reserve is crossed | Actionable 507 error |
| Atomic put | Two callers upload the same bytes | Concurrent store test | No-clobber dedupe | Both receive the same ref |
| Sidecar | Crash after object persist | Partial-state unit test | Re-hash and rebuild sidecar | Transparent recovery |
| Local store | Object was externally corrupted | Corruption unit test | Quarantine, then attempt Fleet | Retry/unavailable if no clean peer |
| Workspace reuse | Same path is initialized for another identity | Binding integration | Quarantine old namespace | New workspace cannot read old assets |
| Origin map | Legacy node has no Runtime ID | Fleet integration | Probe health and atomically backfill | Resolve continues or reports unavailable |
| Origin transfer | Peer stalls mid-body | Idle/overall timeout test | Delete temp and try eligible fallback | Stable unavailable card with Retry |
| Peer integrity | Peer returns wrong or oversized bytes | Mock peer integration | Discard, increment counter, continue | Integrity error if no clean peer |
| Concurrent resolve | Several views request one remote hash | Singleflight integration | One network GET, shared local result | All callers receive the image |
| Browser boundary | Malicious origin POSTs multipart | Router middleware test | Reject before body persistence | HTTP 403; no local mutation |
| Range response | Invalid/multi-range request | Serve integration | Proven range engine returns 416 | Browser receives normal HTTP failure |
| CLI get | Destination exists or download corrupts | CLI integration | Refuse overwrite; temp+hash before rename | Actionable stderr, original file intact |
| Composer | User switches scope during upload | React interaction test | Update captured draft key only | No cross-channel attachment leak |
| Composer | Old completion races a newer draft generation | React interaction test | Ignore stale generation | New files and previews remain intact |
| Composer | Send fails after successful upload | React interaction test | Retain returned refs, skip re-upload | Text/files restored and retryable |
| Renderer | Origin disappears before first resolution | React + Runtime integration | Stable unavailable state | Metadata remains readable; Retry offered |
| WASM mode | Message contains a valid asset ref | Frontend local-mode test | Metadata-only branch | Explicit Runtime-required state |

No identified failure is both silent and lacking test/error handling.

## Performance Review

- Upload and peer download are chunk-streamed; a 50 MiB object is never buffered
  as one Rust byte vector.
- Two concurrent upload requests and free-space checks bound temporary-file and
  descriptor pressure; the persistent quota prevents unbounded workspace growth.
- Local serving uses asynchronous file streaming and one-range responses.
- `loading="lazy"` prevents an opened history from resolving every inline image.
- Demand-driven replicas avoid downloading attachments from unseen channels.
- Per-hash filesystem locks remove duplicate remote bandwidth; a four-transfer
  semaphore bounds cross-hash concurrency.
- Fallback performs concurrency-limited HEAD probes across every eligible peer
  but only one full GET at a time, with explicit peer and whole-resolution
  budgets.
- Sidecars avoid re-hashing on every local read; recovery paths re-hash before
  trusting partial state.
- The object tree is hash-sharded so directory lookup does not degrade into one
  unbounded flat directory.

## Parallelization Strategy

| Step | Modules touched | Depends on |
|---|---|---|
| Protocol and shared fixtures | `gitim-core`, protocol testdata | — |
| Runtime config and Fleet identity | `gitim-runtime` config/Fleet | Protocol |
| Runtime store/resolver/HTTP | `gitim-runtime` assets/HTTP | Protocol, Fleet identity |
| Agent CLI and prompt | `gitim-runtime` CLI, `gitim-agent-provider` prompt | Runtime HTTP |
| Frontend client/draft/renderer | frontend lib/hooks/components | Protocol fixture, Runtime HTTP contract |
| WASM package | `gitim-wasm` package | Protocol |
| Integrated verification | Rust workspace, frontend, local two-node environment | All prior steps |

```text
Lane A: protocol → Runtime config/Fleet → Runtime store/resolver/HTTP → CLI
Lane B: protocol → frontend parser/draft/renderer
Lane C: protocol → WASM rebuild

Merge lanes A+B+C → automated integration → MacBook/Mac mini/Kimi E2E.
```

The Goal executes these as sequential verified milestones in one worktree. The
lane split documents dependency and review boundaries without introducing merge
conflicts in shared manifests or generated WASM output.

## Review Completion Summary

- Step 0 Scope Challenge: full v1 accepted; milestone delivery selected.
- Architecture Review: 1 product issue resolved (demand-driven replication),
  concurrency/security/failure bounds locked.
- Code Quality Review: module boundaries, shared fixtures, typed errors, and
  atomic config updates required.
- Test Review: the protocol, store, HTTP/Fleet, CLI, frontend, WASM, desktop,
  mobile, and two-node matrices are implemented and passing.
- Performance Review: streaming, singleflight, lazy rendering, bounded fallback,
  and global transfer concurrency required.
- NOT in scope: recorded in the requirements document.
- What already exists: recorded above.
- Failure modes: zero silent untested critical gaps.
- Outside voice: the Codex challenge and first independent review identified
  nine concrete merge blockers. Exact-quota retry, cross-process quota,
  Fleet-prefix routing, remote HEAD MIME, workspace draft disposal,
  registration rollback, namespace continuity, browser cache isolation, and
  config-only rollback all have current regressions and are closed.
- Parallelization: three dependency lanes, executed as sequential milestones.
- Lake Score: complete option selected for every engineering coverage decision.
- Full verification: Rust workspace, feature-gated store suite, frontend unit,
  lint, production build, regenerated WASM, Playwright, and final-head release
  binaries passed.
- Live verification: final-head MacBook/Mac mini Fleet transfer, offline
  replica, browser/peer cache split, CLI integrity, Git exclusion, composer,
  and workspace lifecycle isolation passed.
- Unresolved implementation decisions: 0.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|---|---|---|---:|---|---|
| CEO Review | `/plan-ceo-review` | Scope and strategy | 0 | — | Product requirements were approved directly. |
| Codex Challenge | `/codex challenge` | Adversarial implementation audit | 1 | ISSUES CLOSED | Found browser-cache workspace isolation and lifecycle/quota/Fleet gaps; all confirmed findings have regressions. |
| Eng Review | `/plan-eng-review` | Architecture and tests (required) | 1 | CLEAR | 8 issues resolved, 0 critical gaps, 0 unresolved decisions. |
| Independent Review 1 | Task 16 | Full branch implementation gate | 1 + follow-ups | PASS | Zero unresolved P0/P1/P2 after confirmed fixes. |
| Independent Review 2 | Task 16 | Fresh final-context gate | 1 | FOLLOW-UP PENDING | Code audit found no implementation defect; final evidence and audit gates are now updated for re-review. |
| Design Review | `/plan-design-review` | UI/UX gaps | 0 | — | Interaction wireframe approved; visual QA remains in implementation E2E. |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | — | Not required for this feature. |

- **CODEX:** The adversarial challenge is closed by current tests for quotas,
  workspace binding/lifecycle, complete fallback discovery, encoded-ref limits,
  draft generations, peer metadata, and browser cache isolation.
- **CROSS-MODEL:** Both reviews require content integrity, bounded resource use,
  and recoverable failures; the approved demand-driven/Tailnet trust posture is
  preserved.
- **UNRESOLVED:** final documentation-only follow-up review.
- **VERDICT:** IMPLEMENTATION VERIFIED; FINAL REVIEW PENDING.
