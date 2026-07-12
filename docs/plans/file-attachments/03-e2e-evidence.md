# File Attachments: Final-HEAD Two-Node Live E2E Evidence

Date: 2026-07-12 (Asia/Shanghai)

Branch / revision: `codex/file-attachments` /
`15a3fe9c33df53f4964c5d6af9f3fee2eaed8e32`

## Result

The final branch head passed the affected release matrix on an arm64 MacBook
and an arm64 Mac mini. The live run covered browser and peer cache contracts,
fresh Fleet pull-through, offline replica use, Agent CLI integrity, Git text-only
storage, frontend paste/send and picker-preview paths, and workspace
delete/re-register isolation.

Browser-facing GET and HEAD responses were always `private, no-store`.
Node-local peer object GET and HEAD responses remained
`private, immutable, max-age=31536000`, and an actual browser-context request
to the peer-only object route was rejected. Every browser-rendered asset URL
remained on the local Runtime origin; no Fleet peer endpoint or remote node
address reached the DOM or Fetch response URL.

## Final binary provenance

The worktree was clean at the exact revision above before the build.

```text
cargo build --release \
  -p gitim-runtime --bin gitim-runtime \
  -p gitim-cli --bin gitim \
  -p gitim-daemon --bin gitim-daemon \
  --locked
```

The build used stable `rustc 1.94.0` and completed successfully. The same
arm64 artifacts ran on both nodes:

| Binary | Version | SHA-256 |
| --- | --- | --- |
| `gitim-runtime` | `0.9.3` | `281f5753b12a6ce71f1fc0f9463832484ac9264763379c53dbef04df0f80c78f` |
| `gitim` | `0.9.3` | `ba37708cb1f4ea5dfbfa92fc4bd64e818d88875d6067f38de16032ce1cf0ec4e` |
| `gitim-daemon` | `0.9.3` | `4fdc32fe48e361a5841ac6d0372d5807300862d791dfdcd25cc37f1380123786` |

The build artifacts were staged only in a timestamped system temporary
directory. Both copies and all raw run artifacts were removed after the run.

## Topology and stable identity

| Property | MacBook | Mac mini |
| --- | --- | --- |
| Architecture | `arm64` | `arm64` |
| Runtime ID | `24a6489c-762e-4461-9247-a824807a6080` | `3c6a295e-744a-41dc-ba60-5c21bb94e5a2` |
| Final-head feature version | `0.9.3` | `0.9.3` |
| Feature Runtime PID | `56154` | `73359` |
| Workspace | `room` | `room` |
| Workspace identity | `github.com/flame4/room` | `github.com/flame4/room` |

The Runtime IDs remained distinct and stable across feature deployment and
installed-binary restoration. MacBook Fleet status mapped alias `mac-mini` to
the remote `room` workspace with the normalized identity above and reported
`connected` with `retry_count: 0` before the offline test.

The peer transport used the existing local tunnel. Its address and SSH target
are intentionally omitted.

## Browser resolver cache and rendering

Kimi WebBridge used the final frontend from this revision at
`http://127.0.0.1:5173`. An actual page-context Fetch against the MacBook
resolver exercised both GET and HEAD for the existing Mac-mini-origin asset:

```text
origin Runtime ID: 3c6a295e-744a-41dc-ba60-5c21bb94e5a2
sha256: 0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71
GET:  200 image/png, 88510 bytes, Cache-Control: private, no-store
HEAD: 200 image/png, 88510 bytes, Cache-Control: private, no-store
render: complete, 754x714
```

GET and HEAD response URLs, the image `src`, and the enclosing browser origin
all used `127.0.0.1`; the browser result recorded `hasPeerUrl: false`.

The same assertions were repeated for a fresh Mac-mini-origin object during
Fleet pull-through and again after the origin went offline, as described
below.

## Node-local peer contract

Direct node-local GET and HEAD requests to the Mac mini object route for the
existing object both returned:

```text
status: 200
Content-Type: image/png
Content-Length: 88510
Cache-Control: private, immutable, max-age=31536000
ETag: "sha256-0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71"
```

The downloaded bytes hashed to the ETag value. An actual page-context Fetch to
the MacBook node-local object route returned `403` with
`error_code: asset_origin_forbidden`. The rejected response URL was still the
local Runtime URL.

## Fresh Fleet transfer and offline replica

The Mac mini final-head CLI published a fresh deterministic PNG that was absent
from the MacBook store:

```text
sha256: b229fa5c73cfbb0bb12ddacb51ffdb58c6f733656a377846c8729de86c4d3650
size: 54512
dimensions: 879x466
origin Runtime ID: 3c6a295e-744a-41dc-ba60-5c21bb94e5a2
```

Before resolution, MacBook health reported one object and `88510` asset bytes,
and exact object/metadata path checks were absent. A browser HEAD followed by
GET returned `200`, `private, no-store`, the exact size and SHA-256, and a
complete `879x466` image using only the local Runtime URL.

After GET, MacBook health reported two objects and `143022` bytes. The local
object matched the source SHA-256, and its sidecar recorded
`source.kind: fleet_replica` with the Mac mini Runtime ID.

The Mac mini feature Runtime was then stopped. Fleet status became `down` and
the remote health request failed. With the origin offline, browser HEAD, GET,
SHA-256 verification, and `879x466` image rendering all still passed from the
MacBook replica with `private, no-store` and local-only URLs.

The fresh transfer object, metadata, and lock were removed from both nodes
after the proof because no message referenced it.

## Agent CLI integrity and Git storage

Final-head `gitim-runtime asset put` published a local PNG and returned:

```text
sha256: 623b35be5cdac05b8554e02a4cfd0adf81c8f0f326339845da1ebcc6f04bc64a
size: 50464
dimensions: 885x385
```

`gitim-runtime asset get` wrote the canonical ref to a separate destination.
Source, CLI response, and destination SHA-256 values were identical.

The `room` Git checkout was clean after the send. `git grep` found canonical
`<^v1/...>` text refs in `.thread` files. `git ls-files` found no attachment
object path and no path containing any exercised attachment hash. The Runtime
object store remained outside the Git checkout.

The final browser send added one intentional durable message:

```text
dm/cfo--flame4.thread L000091
commit 333110985092de722dc68083b44209c761993d1c
```

Its continuation line contains only the canonical text ref for the
`623b35be...bc64a` object; the PNG bytes are not in Git.

## Final frontend smoke

Kimi WebBridge controlled the real Chrome page for the final frontend build.

- A standard `ClipboardEvent` carrying an image `File` created the paste
  preview.
- Sending cleared the composer draft and remove action.
- Browser network capture recorded `POST /workspaces/room/assets` `200`,
  `POST /workspaces/room/im/send` `200`, and the subsequent local resolver GET
  `200`.
- The sent image rendered complete at `885x385` from the MacBook Runtime URL.
- A standard picker `change` event carrying a separate `FileList` created the
  picker preview. The visible remove action cleared it, while `Attach files`
  remained available. No picker message was sent.

The process-local deletion behavior is covered by the final automated frontend
regressions below: they directly assert draft disposal, blob-URL revocation,
generation invalidation, and isolation of drafts belonging to other
workspaces.

## Workspace delete/re-register isolation

An isolated local workspace used explicit slug `final-head-isolation`.

1. Lifecycle A registered a short temporary path and published a `42299`-byte
   PNG with SHA-256
   `014979bca1be6ae0e8b726988e86ad5c041a8100a0e4a3ed96444f5132821214`.
2. A browser GET returned `200`, `private, no-store`, and the exact SHA-256.
3. DELETE returned `200`; the registry returned `404` for the slug while the
   mode-`0600` config and attachment object remained on disk.
4. Re-registering the same path and slug returned `201`; browser GET again
   returned `200`, `private, no-store`, and the same bytes.
5. After deleting Lifecycle A, Lifecycle B registered a different path with the
   same slug. A default-cache browser request to the identical resolver URL
   returned a fresh `404 asset_missing` response with `private, no-store`.

Lifecycle B was deleted, both temporary workspace trees were removed, and the
Runtime registry again contained only `room`.

## Verification commands

The following final-head checks passed:

```text
cargo build --release -p gitim-runtime --bin gitim-runtime \
  -p gitim-cli --bin gitim -p gitim-daemon --bin gitim-daemon --locked

cargo test -p gitim-runtime --test assets_http \
  node_local_object_rejects_every_browser_context -- --exact
# 1 passed

cargo test -p gitim-runtime --test assets_http \
  local_get_head_range_and_exact_strong_etag_use_verified_metadata -- --exact
# 1 passed

cargo test -p gitim-runtime --test assets_fleet \
  remote_head_honors_exact_strong_validator_and_browser_no_store -- --exact
# 1 passed

cargo test -p gitim-runtime --features test-support --test http_workspaces \
  deleted_local_workspace_assets_survive_failed_and_successful_reregistration \
  -- --exact
# 1 passed

npm exec vitest -- run \
  src/hooks/use-attachment-draft-store.test.ts \
  src/hooks/use-workspace-store.test.ts \
  src/lib/client.local.test.ts
# 3 files passed, 42 tests passed
```

## Mutations and restoration

The run intentionally retained only DM line `L000091`, its Git text ref, and
the referenced MacBook Runtime object. No other persistent workspace mutation
was retained.

Installed binaries were not replaced. Their pre-run SHA-256 values remained:

| Node | Runtime version | Runtime SHA-256 |
| --- | --- | --- |
| MacBook | `0.9.3` | `c91ae7411acaf7688c12d6735b0b80e75d244cc7a34d1ac38407982411ee6639` |
| Mac mini | `0.9.2` | `b63adb2ceafefd8bc4de248ddc2535cd5ec9c3370bd5f75329e08bbf32a4b416` |

Final restoration state:

- MacBook installed Runtime PID `39535`, PPID `1`, serving `127.0.0.1:16868`.
- Mac mini installed Runtime PID `38124`, PPID `1`, serving its original port.
- Fleet tunnel PID `40303` was a child of the MacBook Runtime; Fleet returned
  `connected`, `retry_count: 0`, and the expected `room` mapping.
- Vite, WebBridge network capture, the task tab group, isolated Runtime
  workspaces, and every final-head feature Runtime/daemon were stopped or
  removed.
- Exact process-path audits found no executable remaining under the temporary
  final-head binary directory on either node.
- The unreferenced Fleet test object was removed from both nodes. Mac mini
  returned to its original single object; MacBook retained the original object
  plus the intentional `L000091` object.
- No screenshot, accessibility-tree dump, raw network trace, peer address, or
  authentication material was retained.

## Residual harness boundary

The final-head picker proof exercised the production input `change` path with a
WebBridge-injected browser `FileList`; it did not repeat an OS-native macOS file
panel interaction. The production paste/upload/send/render path, picker
preview/removal path, and deletion draft lifecycle were all exercised at their
affected code boundaries.
