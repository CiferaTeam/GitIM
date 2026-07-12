# File Attachments: Two-Node Live E2E Evidence

Date: 2026-07-12 (Asia/Shanghai)

Branch / starting revision: `codex/file-attachments` /
`bfe3dff5b8c5dfa716830ce7597af0dd3c96e71e`

## Result

The real `room` workspace completed the attachment round trip between an arm64
MacBook and an arm64 Mac mini. The run proved remote-origin storage, Git text
propagation, Fleet pull-through, persistent local replica use while the origin
was offline, WebUI rendering, paste upload/send, picker-path upload/send, Agent
CLI download, mobile rendering, corrupt-peer rejection, and Git exclusion.

Native macOS picker proof also passed: Computer Use opened the real system file
panel, selected the deterministic Downloads PNG, returned to Chrome, and React
rendered the expected preview. The draft was removed without sending.

One harness limitation remains explicit: Kimi WebBridge `upload` reached the
real hidden file input but Chrome rejected `DOM.setFileInputFiles` with
`-32000: Not allowed`. Both a `/tmp` file and a Downloads copy produced the same
Chrome debugger error. No GitIM request was emitted by those failed tool calls.
The same React picker `change` path was therefore completed with a browser
`File` and `DataTransfer` through WebBridge `evaluate`; the subsequent Runtime
upload, IM send, resolver GET, DOM, and screenshots all passed. This is a
WebBridge/Chrome file-injection limitation, not a GitIM product failure. Native
Computer Use independently completed the real system-picker path.

Secrets, PATs, the SSH target, and the Fleet peer base URL are omitted. The
Fleet endpoint is described only as an existing redacted local tunnel.

## Initial topology and restoration ledger

Snapshot time: `2026-07-12T09:10:39Z`.

| Property | MacBook | Mac mini |
| --- | --- | --- |
| Host architecture | `arm64` | `arm64` |
| Runtime ID | `24a6489c-762e-4461-9247-a824807a6080` | `3c6a295e-744a-41dc-ba60-5c21bb94e5a2` |
| Runtime version | `0.9.3` | `0.9.2` |
| Listen address | `127.0.0.1:16868` | `127.0.0.1:16868`, through the existing redacted local tunnel |
| Serving Runtime PID | `2013` | `60020` |
| Runtime binary | `/Users/lewisliu/.gitim/bin/gitim-runtime` | `/Users/lewis/.gitim/bin/gitim-runtime` |
| Runtime command | `gitim-runtime --port 16868` | `gitim-runtime --port 16868` |
| Workspace | `room` at `/Users/lewisliu/ateam/room` | `room` at `/Users/lewis/ateam/room` |
| Workspace identity | `github.com/flame4/room` | `github.com/flame4/room` |

The IDs were distinct. MacBook Fleet alias `mac-mini` mapped local `room` to
remote `room` with normalized identity `github.com/flame4/room`. Initial Fleet
status was `connected`; tunnel PID was `43665`. MacBook also had an older
non-listening Runtime process PID `74703`; it was observed but was not the
server and was not used. No signal was sent to it. Final `ps -p 74703` found it
absent; it exited independently and was never part of the restoration target.

Initial installed binary SHA-256 values:

- MacBook Runtime: `c91ae7411acaf7688c12d6735b0b80e75d244cc7a34d1ac38407982411ee6639`
- Mac mini Runtime: `b63adb2ceafefd8bc4de248ddc2535cd5ec9c3370bd5f75329e08bbf32a4b416`

## Feature binary build and launch

`cargo build --release -p gitim-runtime --bin gitim-runtime -p gitim-cli
--bin gitim --locked` and `cargo build --release -p gitim-daemon --bin
gitim-daemon --locked` passed with stable `rustc 1.94.0`.

Artifacts were copied to timestamped system-temp directories on both nodes:

`/tmp/gitim-file-attachments-e2e-20260712T091308Z/`

| Artifact | SHA-256 |
| --- | --- |
| `gitim-runtime` | `ed35865e564f922bc936e681cc127519118cdebf79cae7ea0134d700f617e5c5` |
| `gitim-daemon` | `9036afe9a65ae0168fd2a1421baea9423934f489ca94bdf394bfc3d1979cebd2` |
| `gitim` | `ba37708cb1f4ea5dfbfa92fc4bd64e818d88875d6067f38de16032ce1cf0ec4e` |

All reported version `0.9.3`. After graceful termination of the original
serving processes, feature Runtime PID `53201` served MacBook and PID `50113`
served Mac mini. Both `/health` responses retained their expected Runtime IDs
and included `asset_store_failures`, `asset_hash_mismatches`,
`asset_fleet_fetch_failures`, `asset_bytes`, `asset_objects`, and
`asset_quota`.

## Remote-origin ref and Git propagation

Fixture:

- Repository source:
  `products/gitim/frontend/public/docs-images/github-token/01-token-basics.png`
- Remote temp copy: `/tmp/gitim-file-attachments-e2e-20260712T091308Z/fixture-origin.png`
- MIME / dimensions / bytes: `image/png`, `754x714`, `88510`
- SHA-256: `0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71`

Mac mini feature CLI `asset put --workspace room` returned:

```text
<^v1/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/sha256:0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71?name=fixture-origin.png&type=image%2Fpng&size=88510&width=754&height=714>
```

The origin object and sidecar were:

```text
/Users/lewis/ateam/room/.gitim-runtime/assets/v1/objects/sha256/05/0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71
/Users/lewis/ateam/room/.gitim-runtime/assets/v1/metadata/sha256/05/0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71.json
```

The sidecar recorded `source.kind: local_upload`. Feature `gitim send` wrote
the ref as `general` line `L000112`, commit
`db86196d4413582ecb50f9b576f894696e184099`. MacBook reached the exact same
HEAD and exact ref without manual fetch.

## Browser Fleet resolution and local replica

Frontend Vite ran at `http://localhost:5173` against the MacBook feature
Runtime. Kimi WebBridge used only session `gitim-file-attachments-e2e` and tab
group `GitIM 附件双节点 E2E`.

The accessibility snapshot found line `112`, link `Open fixture-origin.png`,
and image alt `fixture-origin.png`. Network request `83253.241` was:

```text
GET http://127.0.0.1:16868/workspaces/room/assets/resolve/3c6a295e-744a-41dc-ba60-5c21bb94e5a2/0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71?name=fixture-origin.png
200 image/png
```

Direct response-header verification:

```text
content-type: image/png
content-length: 88510
accept-ranges: bytes
cache-control: private, immutable, max-age=31536000
etag: "sha256-0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71"
x-content-type-options: nosniff
content-disposition: inline; filename="attachment"; filename*=UTF-8''fixture-origin.png
```

The successful Fleet DOM capture returned outer and inner `ok: true`, page
`http://localhost:5173/chat`, `hasPeerTunnelUrl: false`,
`hasRemoteNodeIp: false`, and `assetRoots: 3`. All three asset images were
complete at natural size `754x714`; every image `src` and enclosing link used
only the MacBook resolver at `127.0.0.1:16868`. Evidence:

`/tmp/gitim-file-attachments-e2e-20260712T091308Z/fleet-origin-l112-dom-success.json`

Artifact SHA-256 is
`4f80e7e26fe4d6ff7375f0be3c161a2ca1395ad68315fe05a9ee75215971b54f`.
The L112 image also reported `crossOrigin: anonymous`. The replica was:

```text
/Users/lewisliu/ateam/room/.gitim-runtime/assets/v1/objects/sha256/05/0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71
```

Its SHA-256 matched the origin. Its sidecar recorded
`source.kind: fleet_replica` and origin Runtime ID
`3c6a295e-744a-41dc-ba60-5c21bb94e5a2`.

## Origin-offline replica proof

Mac mini feature PID `50113` stopped gracefully. MacBook Fleet status changed
to `down` with `last_error: remote returned 502 Bad Gateway`; tunnel config was
kept, and the tunnel watcher remained active. With browser cache disabled and
the page reloaded, request `83253.545` fetched the same MacBook-local resolver
URL and returned `200 image/png`.

The image again had natural size `754x714` and used only its MacBook-local
resolver URL. The MacBook object SHA-256 remained
`0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71`.
MacBook `/health` retained `asset_fleet_fetch_failures: 0`, and the Runtime log
showed no asset Fleet request during this local hit. This proves the second load
did not depend on the unavailable origin.

Mac mini restarted as feature PID `78409`; the Runtime ID remained stable. The
existing tunnel was re-established as PID `52990`, and Fleet returned to
`connected` at `2026-07-12T09:21:50.514338Z`.

## Paste and picker interactions

Paste used WebBridge `evaluate` to fetch the deterministic fixture, construct
`File("paste-e2e.png", image/png, 88510)`, add it to `DataTransfer`, and dispatch
a bubbling/cancelable `ClipboardEvent("paste")` on the real message textarea.
React rendered an `86 KiB` preview and `Remove paste-e2e.png`; WebBridge clicked
that button and the next snapshot contained no pasted item.

The separate WebBridge `upload` attempts targeted the actual hidden
`<input multiple hidden type="file">` with a real file. Chrome returned the
harness limitation described in Result. To finish coverage without modifying
product code, WebBridge created `File("picker-e2e.png", image/png, 88510)`, set
it on that same input through `DataTransfer`, and dispatched its normal
bubbling `change` event. React rendered the picker preview.

Text `Task 15 picker attachment E2E` plus the attachment went through the real
composer confirmation and produced:

| Request ID | Request | Result |
| --- | --- | --- |
| `83253.819` | `POST /workspaces/room/assets` | `200 application/json` |
| `83253.820` | `POST /workspaces/room/im/send` | `200 application/json` |
| `83253.821` | `GET /workspaces/room/assets/resolve/24a6489c-.../0558422f...?name=picker-e2e.png` | `200 image/png` |

The UI showed `Sent ✓`, line `L000113`, the text, the rendered image, and
caption `picker-e2e.png / 86 KiB`. Git commit was
`92b0c81666857883dfb806fedd8ce6d347840015`. Mobile emulation recorded
`390x844`; the image rendered at `285x270` with natural size `754x714`.

Screenshots:

| Assertion | Path | SHA-256 |
| --- | --- | --- |
| Origin offline, local replica loaded | `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/origin-offline-loaded.png` | `830057fe25bcb57c67bfcb81103760dbe507007b6d5b2c76b0112471690a728f` |
| Paste preview | `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/paste-preview.png` | `6d85485c86ded3a7ec985e0aa81c3f93fa855b47ecca82fb4ac898c670b1aada` |
| Desktop sent message | `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/desktop-picker-sent.png` | `d938fc1cd2926807516e8994fc9cd951fe3cbf8e94d265dcdccfd8cb07fd0eab` |
| Mobile sent message | `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/mobile-picker-sent.png` | `dcf4993c2c1e350d2194c58e6ea071f3d7507f2e6d3a85a927905085d144246a` |

## Follow-up evidence

### Paste-origin upload and send

WebBridge constructed `File("paste-send-e2e.png", image/png, 88510)`, attached
it to `DataTransfer`, and dispatched a bubbling/cancelable
`ClipboardEvent("paste")` on the real textarea. The event reported
`defaultPrevented: true`, and React rendered the preview before send.

Text `Task 15 paste attachment E2E` plus that pasted file completed the normal
composer confirmation path:

| Request ID | Request | Result |
| --- | --- | --- |
| `83253.2204` | `POST /workspaces/room/assets` | `200 application/json` |
| `83253.2205` | `POST /workspaces/room/im/send` | `200 application/json` |
| `83253.2206` | `GET /workspaces/room/assets/resolve/24a6489c-.../0558422f...?name=paste-send-e2e.png` | `200 image/png` |

The result was `general` line `L000114`, commit
`e8c856ddcd1f319dfc901155cbbd41f1f8b54077`, with canonical ref:

```text
<^v1/24a6489c-762e-4461-9247-a824807a6080/sha256:0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71?name=paste-send-e2e.png&type=image%2Fpng&size=88510&width=754&height=714>
```

The accessibility snapshot showed `Sent ✓`, text, rendered image, filename,
and `86 KiB` caption. Evidence:

- Ready screenshot:
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/paste-send-ready.png`,
  SHA-256 `5e2043aa1d439c52f0004c4a3c988946c5c7d55e0466fd37649028450e4e59c5`.
- Sent screenshot:
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/paste-send-sent.png`,
  SHA-256 `b8f025c2eeb573adc24f03a7ac43c380646ab4784dc8a2f735d89f73389028b2`.
- Network capture:
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/network-paste-send-final.json`.

### Correctly framed Fleet-origin image

The corrected desktop screenshot visibly contains `L112`, its rendered
`fixture-origin.png`, filename, and `86 KiB` caption:

`/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/fleet-origin-l112-visible.png`

Screenshot SHA-256 is
`157a4f0b9598da675a12c7aab0a92c438118789a6275f9516f026b548b84c013`.
Resolver request `83253.2447` returned `200 image/png` from the MacBook-local URL
whose origin segment is the Mac mini Runtime ID. The successful DOM artifact
above recorded three complete `754x714` images, `hasPeerTunnelUrl: false`,
`hasRemoteNodeIp: false`, and local-resolver-only link/source URLs. The MacBook
replica object remained SHA-256
`0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71`.
Network detail is in
`/tmp/gitim-file-attachments-e2e-20260712T091308Z/network-fleet-origin-l112.json`.

### Isolated corrupt-peer rejection

This assertion used disposable workspace `/tmp/gfa-cp.KAzWyR/w`, isolated
Runtime ID `bf98eaac-654d-428a-9a90-7bb01c79b4d2`, and a local mock peer with
Runtime ID `7d31fa58-398e-4ed7-a879-6f8d32fa62ce`. It never registered or
resolved through `room`.

The requested SHA-256 was
`6433d30c180f5643d155ae78deb0e1d7fc610559cda7e9a2a10d880f52574724`.
The peer served `wrong origin bytes`, whose actual SHA-256 was
`2dca366f8d6b5f6aaa5406d746adf3018869b8850bfa00b47dc29639d02e7f01`,
while presenting immutable-object headers for the requested hash.

The isolated Runtime returned HTTP `502 Bad Gateway`:

```json
{"ok":false,"error":"asset hash mismatch","error_code":"asset_hash_mismatch"}
```

`/health` changed `asset_hash_mismatches` from `0` to `1` and
`asset_fleet_fetch_failures` from `0` to `1`; `asset_bytes` and
`asset_objects` remained `0`. Store inspection found no object, metadata, or
temporary file. The only hash-named artifact was the designed zero-byte lock
anchor. The mock log recorded one object GET, and the Runtime log emitted both
`asset_hash_mismatch` and `asset_fleet_fetch_failure`.

Exact evidence paths:

- `/tmp/gfa-cp.KAzWyR/corrupt-response-headers.txt`
- `/tmp/gfa-cp.KAzWyR/corrupt-response-body.json`
- `/tmp/gfa-cp.KAzWyR/health-before.json`
- `/tmp/gfa-cp.KAzWyR/health-after.json`
- `/tmp/gfa-cp.KAzWyR/mock-peer-requests.log`
- `/tmp/gfa-cp.KAzWyR/h/.gitim/logs/runtime.log`

The isolated Runtime and mock peer were stopped after collection. A product-UI
unavailable screenshot was not attempted because it would require switching
the prepared live frontend to a different Runtime port and disposable
workspace; the HTTP error, counters, logs, and clean store provide the isolated
security assertion without risking `room` or the prepared native-picker page.

### Native macOS picker

Computer Use clicked the real WebUI `Attach files` button, opened the macOS
native `打开` panel, used Go to Folder for the deterministic file, selected the
actual PNG shown by Finder as `754x714`, and clicked native `打开`. Chrome
returned to GitIM with preview text
`GitIM-Task15-native-picker-0558422f.png / 86 KiB` and button
`Remove GitIM-Task15-native-picker-0558422f.png`.

Independent Computer Use provenance:

- Native panel screenshot
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/native-picker-panel-selected.jpeg`,
  SHA-256 `d7692f9f76d8c0353d3b04dd807b4271824e6493bc9f07d2271045bbca8cd528`.
  It visibly shows the selected Downloads PNG, Finder preview, and enabled
  native `打开` button.
- Native panel accessibility tree
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/native-picker-computer-use-panel.txt`,
  SHA-256 `b58e17a344f4a6fc63eaf2637d4e4f767647d682969a897846db2a40c8781cdf`.
  It records native Window `打开`, the selected Downloads file, Finder preview
  metadata `754x714`, and enabled `打开` button.
- Post-open Chrome screenshot
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/native-picker-computer-use-preview.jpeg`,
  SHA-256 `331cbd3662aa793d0e777174753604f90c0ac0b0f197807a42eda0188907aaab`.
- Post-open accessibility tree
  `/tmp/gitim-file-attachments-e2e-20260712T091308Z/native-picker-computer-use-preview.txt`,
  SHA-256 `e3687b432a368ee586e85f1906d7f2237cba4c3ee0eb85a7be4cfb04ddb4dcfe`.
  It records the `86 KiB` preview and its exact remove action. These artifacts
  independently establish native file selection before the Kimi snapshot.

Selected file:

`/Users/lewisliu/Downloads/GitIM-Task15-native-picker-0558422f.png`

It is `88510` bytes with SHA-256
`0558422fb26ac353e279c97762f8d2eb22a5cecbc8a4e8627deb38c9479f6c71`.
The before-selection browser snapshot is
`/tmp/gitim-file-attachments-e2e-20260712T091308Z/native-picker-ready-snapshot.json`.
The resulting accessibility snapshot is
`/tmp/gitim-file-attachments-e2e-20260712T091308Z/native-picker-preview-snapshot.json`.
The visible preview screenshot is:

`/tmp/gitim-file-attachments-e2e-20260712T091308Z/screenshots/native-picker-preview.png`

It is `133549` bytes with SHA-256
`7c2f233c4171a23ac1b26ed3e4955d6bf4fdc195c65290bf5e3bf7c186615b2c`.
No send was performed. WebBridge then clicked the exact remove button; the
post-removal snapshot
`/tmp/gitim-file-attachments-e2e-20260712T091308Z/native-picker-after-remove-snapshot.json`
contains neither the filename nor its remove action and still contains
`Attach files`.

## Agent CLI and Git exclusion proof

Feature `gitim-runtime asset get` ran for both origin refs on both nodes:

```text
Mac mini-origin ref -> get-mini-ref.png     88510 bytes  sha256 0558422f...6c71
MacBook-origin ref  -> get-macbook-ref.png  88510 bytes  sha256 0558422f...6c71
```

All four destinations exactly matched the source hash.

Attachment-commit audit through
`e8c856ddcd1f319dfc901155cbbd41f1f8b54077` found:

- `db86196d`: only `channels/general.thread`, one insertion.
- `92b0c816`: only `channels/general.thread`, two insertions.
- `e8c856dd`: only `channels/general.thread`, two insertions.
- `git ls-tree -r`, `git ls-files`, and both commit changed-path lists contained
  no asset/object/metadata/temp directory and no PNG/JPEG/GIF/WebP/AVIF file.
- `rg '<\^v1/' channels dm` found only the three canonical text refs at
  `general.thread:383`, `general.thread:385`, and `general.thread:387`.
- At final verification both human clones were clean and converged on
  `ce648f5b00e02f9a9c302d4a2ceef24aa9776469`; commits after `e8c856dd` were
  the workspace's existing cron activity, not additional attachment writes.

## Mutations and final restoration

Persistent test mutations intentionally retained:

- Three `general` messages/commits, `L000112`, `L000113`, and `L000114`.
- Content-addressed Runtime objects/sidecars on both nodes.
- Timestamped `/tmp/gitim-file-attachments-e2e-20260712T091308Z/` evidence,
  binaries, downloads, logs, network captures, and screenshots.
- Disposable corrupt-peer evidence under `/tmp/gfa-cp.KAzWyR/`; its Runtime
  and mock-peer processes are stopped.
- One open WebBridge tab group for user inspection.

Final restoration after the native picker proof:

- The unsent native-picker draft was removed and verified absent.
- WebBridge network capture stopped; Vite stopped; port `5173` no longer
  listens. The WebBridge group remains open.
- MacBook restored installed Runtime `0.9.3` as PID `92147`, PPID `1`, command
  `/Users/lewisliu/.gitim/bin/gitim-runtime --port 16868`.
- Mac mini restored installed Runtime `0.9.2` as PID `20137`, PPID `1`, command
  `/Users/lewis/.gitim/bin/gitim-runtime --port 16868`.
- Final Runtime IDs match the initial values. Runtime status reports `room`,
  four MacBook agents, and six Mac mini agents.
- Final tunnel PID `96731` is healthy. Fleet returned to `connected` at
  `2026-07-12T11:57:35.874748Z`.
- Exact-path process audits found no feature Runtime or feature daemon left on
  either node.
- The native Downloads fixture was removed after its screenshot and snapshot
  were preserved.
- The disposable corrupt Runtime and mock peer remain stopped; ports `16968`
  and `16969` are closed.

No product source files were changed during this live E2E.
