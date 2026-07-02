# Quick Session Cards Implementation Plan

Status: Draft
Date: 2026-07-02

## Overview

Quick Session Cards introduce a lightweight top-level conversation layer that is persisted by GitIM, executed by the runtime, and surfaced in the WebUI as a hoverable quick session hub.

The implementation spans:

- `gitim-core`: shared quick session types and ref parsing.
- `gitim-daemon`: durable object storage, validation, and cross-node git sync.
- `gitim-client`: IPC wrappers.
- `gitim-runtime`: provider execution, title API gate, per-session state, queueing, scoped events, and HTTP endpoints.
- `products/gitim/frontend`: top hub UI, store, client methods, event routing, and ref drag/drop.

**Cross-node model:** Quick session persisted objects (`session.meta.yaml`, `discussion.thread`) are git-synced like cards/channels/DMs. Any node can create or read them. Execution runs on the node that owns the selected agent. Provider session state (tokens, compaction, streaming) remains local to the executing runtime.

## Phase X.0: Refined Plan — Cross-Node Dispatch, Session IDs, Ref Disambiguation

Resolves three High P1 items from plan-review `20260702-143803-067` before production implementation starts.

### P1-1: Cross-Node Dispatch

**Detection:** Daemon poll (`handle_poll`) already diff-detects new files by path prefix. Add `quick-sessions/<id>/` path detection alongside existing `channels/`, `dm/`, `crons/` branches:
- When `quick-sessions/<id>/session.meta.yaml` appears and `agent_id` matches a handler this daemon hosts → emit `kind: "quick_session_meta"` in poll response
- When `quick-sessions/<id>/discussion.thread` changes → emit `kind: "quick_session_thread"` with parsed entries

**Runtime dispatch:** Runtime receives poll changes from daemon. On `quick_session_meta`:
- If `status == needs_title` and `agent_id` matches current agent → dispatch a turn through `quick_session_runner`
- The turn executes on this node's runtime, writes responses to `discussion.thread`, commits

**Poll frequency:** Uses existing poll cycle — client polls daemon (HTTP GET), daemon git-diffs since last cursor. No additional timer needed.

**Stale/offline handling:**
- If agent's node is offline: meta.yaml exists (git-synced) but runtime never picks it up → session stays `needs_title`
- After node comes online → next poll detects the new session → dispatched
- Optional: after session has been `needs_title` for > 30 min with no agent response, daemon on initiating node marks `status = error` with reason `"agent_unreachable"`

**Retry/timeout:**
- Per-agent work queue serializes turns; no explicit retry beyond agent_loop normal resume-on-poll
- If provider call fails → standard error flow (error event, status update)

### P1-2b/P1-5c: Session ID Generation

**Algorithm:** ULID (Universally Unique Lexicographically Sortable Identifier)
- 26 characters, Crockford base32 (`0123456789ABCDEFGHJKMNPQRSTVWXYZ`)
- 128-bit: 48-bit timestamp (ms) + 80-bit random
- Collision probability: ~2^-80 per pair, effectively zero
- Sortable by creation time (lex order = chronological)
- URL-safe, no special characters

**Format:** `qs-<ulid>` (29 chars total), e.g., `qs-01ARZ3NDEKTSV4RRFFQ69G5FAV`

**Validation regex:** `^qs-[0-9A-HJKMNP-TV-Z]{26}$` (Crockford base32 excludes I,L,O,U)

**Uniqueness across nodes:** ULID's 80-bit random component provides cross-node uniqueness without coordination.

**Collision handling:** For defense-in-depth, daemon's `create_quick_session` handler checks if `quick-sessions/<id>/` already exists (via `std::fs` or git tree). On collision, regenerate ULID up to 3 times. If all 3 attempts collide, return typed error `QUICK_SESSION_ID_COLLISION`. This keeps the invariant explicit without adding meaningful overhead.

**Dependency:** Add `ulid` crate to `gitim-core` Cargo.toml, or implement a minimal ULID generator (~50 lines) to avoid dependency.

### P1-5a: Ref Disambiguation

**Syntax:** `session:<id>` is bare text (no `<` `>` markers), distinct from existing `<#...>` link syntax.

**Parser:** Add `SESSION_REF_RE` to `link.rs`:
```regex
\bsession:(qs-[0-9A-HJKMNP-TV-Z]{26})(:L(\d{6,}))?\b
```
- Matches `session:qs-<ulid>` (bare) and `session:qs-<ulid>:L000001` (with line ref)
- Word-boundary anchors prevent partial matches (e.g., `x-session:qs-xxx` not matched)
- No conflict with existing `<#channel/card-id>` parser — different prefix entirely

**LinkKind:** Add variant:
```rust
QuickSession {
    session_id: String,
    line_number: Option<u64>,
}
```

**Line refs in v1:** Yes. The `session:<id>:L000001` format is supported to mirror `<#channel:L000042>` message links. Frontend reference preview resolves the referenced line.

**No marker wrapping:** Quick session refs do not use `<` `>` markers. Unlike `<#channel>`, `<~user>`, `<!url>`, the `session:` prefix is self-delimiting and unambiguous. The word-boundary regex prevents false positives in prose.

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

- `crates/gitim-core/src/types/quick_session.rs` — new: types + `validate_quick_session_id()`
- `crates/gitim-core/src/types/mod.rs` — re-export quick_session module
- `crates/gitim-core/src/link.rs`
- `crates/gitim-core/src/parser.rs`
- `crates/gitim-core/src/formatter.rs`

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
4. Inject title API gate instruction into the agent's system prompt for quick session turns.
5. Run quick session turns through the same provider abstraction as the main agent loop.
6. Enforce title API gate: if the agent sends assistant content before calling `set_quick_session_title`, return a typed error and reject the turn.
7. Serialize main-agent and quick-session turns per agent through `agent_work_queue`.
8. Dispatch quick session turns for cross-node agents: detect new sessions via daemon poll, route to the local runtime that hosts the agent.
9. Persist user and agent messages to the quick session thread through daemon/client APIs.
10. Update quick session status during needs_title/queued/running/error/idle transitions.

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

## Phase 5: Title API Gate And Compression

Implement the title API gate (agent must set title before replying) and compression lifecycle.

Files:

- `crates/gitim-runtime/src/quick_session_runner.rs`
- `crates/gitim-runtime/src/quick_session_state.rs`
- `crates/gitim-daemon/src/quick_session_handlers.rs`
- `crates/gitim-runtime/src/http.rs` (title endpoint)

Work:

1. New sessions are created with `status = needs_title` and `title_source = none`.
2. Expose `POST /workspaces/{slug}/quick-sessions/{id}/title` endpoint accepting `{ title: string }`.
3. Runtime enforces: if agent attempts to send assistant content for a session where `status == needs_title`, return typed error `QUICK_SESSION_TITLE_REQUIRED`.
4. On successful `set_quick_session_title` call, update `title`, `title_source = api_set`, `status = active`.
5. Inject a prompt instruction for quick session turns: "Before your first reply, call `set_quick_session_title` with a short title (max 80 chars) that summarizes this session."
6. Allow subsequent `set_quick_session_title` calls to update the title (e.g., agent refines mid-session).
7. Apply token estimate and usage tracking per quick session.
8. On compaction/reset, write the quick session summary to metadata and clear only that quick session state.
9. Restore future quick session turns from transcript plus summary.

Verification:

- `cargo test -p gitim-runtime quick_session_title_gate`
- `cargo test -p gitim-runtime quick_session_compaction`

## Phase 6: Runtime HTTP API

Expose quick sessions to the WebUI.

Files:

- `crates/gitim-runtime/src/http.rs`
- `crates/gitim-runtime/src/http_types.rs` if the endpoint types are split during implementation

Work:

1. Add create/list/read/send/title/update/archive/unarchive HTTP endpoints.
2. Return typed errors with stable `error_code` values including `QUICK_SESSION_TITLE_REQUIRED`.
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

1. Add an end-to-end test covering create, title gate enforcement, stream, archive, and drag-reference.
2. Add a cross-node E2E test: create session on node A targeting agent on node B; verify node B's runtime executes the turn and node A receives the response.
3. Remove demo-only quick session mock files and imports.
4. Check frontend at desktop and narrow widths for overlap and text fit.
5. Check quick session state files are isolated from main agent state.
6. Check two sessions on the same agent apply events only to their matching `session_id`.
7. Check title gate: agent that sends assistant content before setting title receives typed error; agent that sets title first proceeds normally.

Verification:

- `cargo test`
- `npm --prefix products/gitim/frontend run build`
- E2E browser test for quick session hub

## Key Risks

1. Provider concurrency through one agent profile can corrupt hidden provider state. The per-agent work queue is required before runtime sends real quick session turns.
2. Title API gate can break if the agent ignores or fails to parse the `set_quick_session_title` prompt instruction. The typed error path must be user-visible so the human knows the session stalled due to missing title, not a provider error.
3. Event routing can regress existing agent panels. Scoped events should be optional and backward-compatible.
4. Compression can accidentally clear the main agent session if state factoring is too broad. Tests should assert main and quick session state files independently.
5. Cross-node sessions risk stale state if the agent's node is offline when the session is created. The initiating node must handle the gap between creation and first response gracefully (status remains `needs_title` until the agent node processes the turn).
6. Drag/drop can create surprising hidden side effects. The browser smoke test should verify that dragging only inserts a ref and the actual GitIM write occurs when the target message is sent.

## Review Checklist

Before implementation starts, confirm:

- `session:qs-<ulid>` (29 chars, ULID, Crockford base32) is the quick session ref format. Line refs (`:L000001`) supported.
- Quick session files live under `quick-sessions/<id>/`.
- The top hub is the canonical entry point for v1.
- Per-agent queueing is acceptable for v1 latency.
- Title is set by the agent via `set_quick_session_title` API gate before first assistant response.
- Cross-node dispatch uses existing poll mechanism: daemon detects `quick-sessions/<id>/` paths in git diff, routes to runtime when `agent_id` matches.
- Cross-node sessions use git-synced daemon objects; no provider state serialization across nodes.
