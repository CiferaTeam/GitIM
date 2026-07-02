# Quick Session Cards Implementation Plan

Status: Draft
Date: 2026-07-02

## Overview

Quick Session Cards introduce a lightweight top-level conversation layer that is persisted by GitIM, executed by the runtime, and surfaced in the WebUI as a hoverable quick session hub.

The implementation spans:

- `gitim-core`: shared quick session types and ref parsing.
- `gitim-daemon`: durable object storage and validation.
- `gitim-client`: IPC wrappers.
- `gitim-runtime`: provider execution, per-session state, queueing, scoped events, and HTTP endpoints.
- `products/gitim/frontend`: top hub UI, store, client methods, event routing, and ref drag/drop.

## Phase 0: Baseline And Demo Inventory

1. Record current test baseline before feature work:
   - `cargo test`
   - `npm --prefix products/gitim/frontend run build`
2. Keep the existing fake UI demo files as reference during implementation:
   - `products/gitim/frontend/src/components/sessions/`
   - `products/gitim/frontend/src/lib/session-demo-dnd.ts`
   - Current `app-shell.tsx` and `sidebar.tsx` demo wiring
3. Replace the demo wiring during the frontend phase after real APIs exist.

## Phase 1: Core Types And Reference Parsing

Add shared quick session protocol types in `gitim-core`.

Files:

- `crates/gitim-core/src/types.rs`
- `crates/gitim-core/src/link.rs`
- `crates/gitim-core/src/parser.rs`
- `crates/gitim-core/src/formatter.rs`
- `crates/gitim-core/src/validator.rs`

Work:

1. Add `QuickSession`, `QuickSessionStatus`, `QuickSessionTitleSource`, and request/response payload types.
2. Validate quick session ids with a stable generated prefix such as `qs-`.
3. Add `session:<id>` quick session ref recognition in `gitim-core::link`.
4. Extend parser/formatter tests so quick session refs round-trip inside normal message bodies.

Verification:

- `cargo test -p gitim-core quick_session`
- `cargo test -p gitim-core link`

## Phase 2: Daemon Storage And IPC

Add daemon-owned persisted object operations.

Files:

- `crates/gitim-daemon/src/quick_session_handlers.rs`
- `crates/gitim-daemon/src/handlers/mod.rs`
- `crates/gitim-daemon/src/handlers/poll.rs`
- `crates/gitim-client/src/lib.rs`
- Shared IPC request/response definitions used by daemon and client

Work:

1. Create `quick-sessions/<id>/session.meta.yaml`.
2. Create and append `quick-sessions/<id>/discussion.thread` with existing GitIM line format.
3. Implement list/read/create/send/update/archive/unarchive handlers.
4. Include quick session changes in poll responses where the frontend needs cheap refresh signals.
5. Add client methods mirroring daemon operations.
6. Preserve daemon write validation as the source of truth for persisted quick session objects.

Verification:

- `cargo test -p gitim-daemon quick_session`
- `cargo test -p gitim-client quick_session`

## Phase 3: Runtime Session State And Execution

Add runtime execution for quick session turns.

Files:

- `crates/gitim-runtime/src/http.rs`
- `crates/gitim-runtime/src/agent_loop.rs`
- `crates/gitim-runtime/src/state.rs`
- `crates/gitim-runtime/src/quick_session_state.rs`
- `crates/gitim-runtime/src/quick_session_runner.rs`
- `crates/gitim-runtime/src/agent_work_queue.rs`

Work:

1. Factor provider session state fields from `AgentState` into a reusable session-state shape.
2. Store quick session runtime state at `.gitim-runtime/quick-sessions/<id>.state.json`.
3. Build quick session provider config from the selected agent's provider/model/system prompt/env/profile.
4. Run quick session turns through the same provider abstraction as the main agent loop.
5. Serialize main-agent and quick-session turns per agent through `agent_work_queue`.
6. Persist user and agent messages to the quick session thread through daemon/client APIs.
7. Update quick session status during queued/running/error/idle transitions.

Verification:

- `cargo test -p gitim-runtime quick_session_state`
- `cargo test -p gitim-runtime quick_session_runner`
- `cargo test -p gitim-runtime agent_work_queue`

## Phase 4: Scoped Runtime Events

Extend activity events so frontend consumers can route each event to a single owner.

Files:

- `crates/gitim-runtime/src/http.rs`
- `crates/gitim-runtime/src/agent_loop.rs`
- `products/gitim/frontend/src/hooks/use-agent-activity.ts`
- `products/gitim/frontend/src/lib/types.ts`

Work:

1. Add optional `scope`, `session_id`, and `ref` to `AgentActivityEvent`.
2. Emit `scope = "agent_main"` for normal agent-loop events.
3. Emit `scope = "quick_session"` for quick session turns.
4. Keep main activity consumers compatible with missing scope.
5. Route quick session events into the quick session store by `session_id`.
6. Route usage snapshots so quick session usage updates patch quick session state.

Verification:

- `cargo test -p gitim-runtime activity_event`
- Frontend store unit tests for main event and quick session event routing.

## Phase 5: Title Generation And Compression

Implement the two lifecycle behaviors that make quick sessions feel complete.

Files:

- `crates/gitim-runtime/src/quick_session_runner.rs`
- `crates/gitim-runtime/src/quick_session_state.rs`
- `crates/gitim-daemon/src/quick_session_handlers.rs`

Work:

1. Save a deterministic provisional title from the first message.
2. Generate a short title after the first agent response when provider execution succeeds.
3. Save `title_source = generated` after the generated title is accepted.
4. Apply token estimate and usage tracking per quick session.
5. On compaction/reset, write the quick session summary to metadata and clear only that quick session state.
6. Restore future quick session turns from transcript plus summary.

Verification:

- `cargo test -p gitim-runtime quick_session_title`
- `cargo test -p gitim-runtime quick_session_compaction`

## Phase 6: Runtime HTTP API

Expose quick sessions to the WebUI.

Files:

- `crates/gitim-runtime/src/http.rs`
- `crates/gitim-runtime/src/http_types.rs` if the endpoint types are split during implementation

Work:

1. Add create/list/read/send/update/archive/unarchive HTTP endpoints.
2. Return typed errors with stable `error_code` values.
3. Include the stable `ref` in create/read/list responses.
4. Return hub-list metadata directly from list responses.
5. Keep endpoint auth and workspace slug behavior aligned with existing runtime workspace APIs.

Verification:

- `cargo test -p gitim-runtime quick_session_http`
- Manual `curl` smoke against a local runtime.

## Phase 7: Frontend Data Layer

Replace demo-only data with real API/state.

Files:

- `products/gitim/frontend/src/lib/types.ts`
- `products/gitim/frontend/src/lib/client.ts`
- `products/gitim/frontend/src/hooks/use-quick-session-store.ts`
- `products/gitim/frontend/src/hooks/use-agent-activity.ts`

Work:

1. Add quick session API types.
2. Add client methods for runtime endpoints.
3. Add a Zustand store for list, selected session, transcript, pending sends, archive state, and scoped event application.
4. Load active sessions when the hub opens.
5. Lazy-load transcript when a session is selected.
6. Apply scoped runtime events by `session_id`.

Verification:

- `npm --prefix products/gitim/frontend run build`
- Frontend unit tests for store reducers if the project test setup is active.

## Phase 8: Frontend Hub UI

Promote the fake demo interaction into the real WebUI.

Files:

- `products/gitim/frontend/src/components/layout/app-shell.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-hub.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-list.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-panel.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-composer.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-row.tsx`

Work:

1. Place the top-level entry near the current navigation tabs.
2. Open the hub on hover and keep it open while focused or clicked.
3. Show active quick sessions aggregated across agents.
4. Provide an agent picker and first-message composer.
5. Open the selected session in a compact side panel inside the floating hub.
6. Add archive controls in row and panel contexts.
7. Use `DESIGN.md` tokens for color, spacing, typography, density, and radius.
8. Remove demo-only state and mock rows once real data is wired.

Verification:

- `npm --prefix products/gitim/frontend run build`
- Browser smoke: hover, create, select, send, archive.

## Phase 9: Reference Preview And Drag/Drop

Wire quick session refs into existing message composition and preview surfaces.

Files:

- `products/gitim/frontend/src/components/chat/message-parser.ts`
- `products/gitim/frontend/src/components/chat/message-body.tsx`
- `products/gitim/frontend/src/components/chat/reference-preview.tsx`
- `products/gitim/frontend/src/components/sessions/quick-session-dnd.ts`
- Existing channel/card composer components

Work:

1. Parse `session:<id>` refs in message bodies.
2. Resolve quick session refs in reference preview.
3. Add drag payload from quick session rows and ref tokens.
4. Accept quick session drag payloads in channel/card composers.
5. Insert the stable ref text into the composer at the drop/cursor position.

Verification:

- Frontend parser tests for quick session refs.
- Browser smoke: drag from hub into channel/card composer and send.

## Phase 10: End-To-End QA And Cleanup

Run focused and full checks after implementation.

Work:

1. Add an end-to-end test covering create, stream, archive, and drag-reference.
2. Remove demo-only quick session mock files and imports.
3. Check frontend at desktop and narrow widths for overlap and text fit.
4. Check quick session state files are isolated from main agent state.
5. Check two sessions on the same agent apply events only to their matching `session_id`.

Verification:

- `cargo test`
- `npm --prefix products/gitim/frontend run build`
- E2E browser test for quick session hub

## Key Risks

1. Provider concurrency through one agent profile can corrupt hidden provider state. The per-agent work queue is required before runtime sends real quick session turns.
2. Event routing can regress existing agent panels. Scoped events should be optional and backward-compatible.
3. Compression can accidentally clear the main agent session if state factoring is too broad. Tests should assert main and quick session state files independently.
4. Drag/drop can create surprising hidden side effects. The browser smoke test should verify that dragging only inserts a ref and the actual GitIM write occurs when the target message is sent.

## Review Checklist

Before implementation starts, confirm:

- `session:<id>` is the accepted quick session ref format.
- Quick session files live under `quick-sessions/<id>/`.
- The top hub is the canonical entry point for v1.
- Per-agent queueing is acceptable for v1 latency.
- Generated titles can start with a deterministic first-message fallback.
