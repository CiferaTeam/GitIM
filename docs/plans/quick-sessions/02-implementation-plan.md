# Quick Sessions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship Git-synced, agent-isolated Quick Sessions with native and browser writers, scoped runtime execution, a top-nav conversation hub, and stable references.

**Architecture:** Quick Sessions are canonical GitIM objects whose metadata transitions live in `gitim-core`. Native and browser daemons write identical metadata/thread commits. The existing per-agent `AgentLoop` is the only provider scheduler and executes queued Quick Sessions sequentially with isolated runtime state. Frontend state refreshes from the existing poll/SSE channels and rejects stale revisions.

**Tech Stack:** Rust stable, Tokio, serde/serde_yaml, gitim daemon/client/runtime/provider crates, wasm-bindgen, React 19, Zustand, Radix UI, Vitest, Playwright.

---

## Execution invariants

```text
Git object revision                 Local provider generation
-------------------                 -------------------------
meta.revision increments            state.context_generation increments
on every durable mutation           only when quick context resets
          |                                    |
          +------ event carries both ----------+
                            |
                            v
              frontend accepts only newest
```

- All work happens in `/Users/lewisliu/ateam/GitIM/.codex/draft-48`.
- Each production behavior starts with a failing test and a recorded RED result.
- Every commit uses Conventional Commits, a `Test:` footer, and `Co-authored-by: Codex <codex@openai.com>`.
- Rust checks use the repository's stable toolchain and scoped test commands.
- Core protocol changes rebuild and commit `crates/gitim-wasm/pkg/`.

### Task 1: Canonical Quick Session protocol and wasm transitions

**Files:**

- Create: `crates/gitim-core/src/types/quick_session.rs`
- Modify: `crates/gitim-core/src/types/mod.rs`
- Modify: `crates/gitim-core/src/types/link.rs`
- Modify: `crates/gitim-core/src/link.rs`
- Modify: `crates/gitim-core/src/responses.rs`
- Modify: `crates/gitim-wasm/src/lib.rs`
- Regenerate: `crates/gitim-wasm/pkg/`

- [ ] **Step 1: Write failing core tests for ids, metadata, transitions, idempotency, and refs**

Add tests that construct the public API below and assert:

```rust
let mut meta = QuickSessionMeta::new(
    "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
    "alice".to_string(),
    "lewis".to_string(),
    "2026-07-11T00:00:00Z".to_string(),
);
assert_eq!(meta.status, QuickSessionStatus::NeedsTitle);
assert_eq!(meta.revision, 1);

apply_quick_session_transition(
    &mut meta,
    QuickSessionTransition::Claim {
        actor: "alice".to_string(),
        input_line: 1,
        attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        now: "2026-07-11T00:00:01Z".to_string(),
    },
)?;
assert_eq!(meta.status, QuickSessionStatus::Running);

let duplicate = apply_quick_session_transition(
    &mut meta,
    QuickSessionTransition::AgentReply {
        actor: "alice".to_string(),
        input_line: 1,
        attempt_id: "qa-01JZZZZZZZZZZZZZZZZZZZZZZZ".to_string(),
        output_line: 2,
        preview: "done".to_string(),
        now: "2026-07-11T00:00:02Z".to_string(),
    },
)?;
assert_eq!(duplicate, TransitionOutcome::Applied);
```

Also assert invalid ids, wrong actors, stale attempts, reply-before-title, archive restoration, duplicate create/reply outcomes, Unicode title limits, `session:<id>` parsing, valid preceding boundaries, and optional `:L000001`.

- [ ] **Step 2: Run core tests and verify RED**

Run:

```bash
cargo test -p gitim-core quick_session --locked
cargo test -p gitim-core link --locked
```

Expected: compilation fails because `QuickSessionMeta`, transition types, and `LinkKind::QuickSession` do not exist.

- [ ] **Step 3: Implement the protocol types and pure transition function**

Implement these public shapes:

```rust
pub const QUICK_SESSION_TITLE_MAX_CHARS: usize = 80;
pub const QUICK_SESSION_SUMMARY_MAX_CHARS: usize = 4_000;
pub const QUICK_SESSION_PREVIEW_MAX_CHARS: usize = 160;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionStatus {
    NeedsTitle,
    Running,
    Active,
    Error,
    Archived,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuickSessionTitleSource {
    None,
    ApiSet,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QuickSessionMeta {
    pub id: String,
    pub title: Option<String>,
    pub title_source: QuickSessionTitleSource,
    pub agent_id: String,
    pub created_by: String,
    pub status: QuickSessionStatus,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
    pub archived_from: Option<QuickSessionStatus>,
    pub summary: Option<String>,
    pub summary_updated_at: Option<String>,
    pub last_message_preview: String,
    pub error: Option<String>,
    pub processing_input_line: Option<u64>,
    pub processing_started_at: Option<String>,
    pub attempt_id: Option<String>,
    pub last_completed_attempt_id: Option<String>,
    pub last_completed_input_line: Option<u64>,
    pub last_completed_line: Option<u64>,
    pub last_failed_attempt_id: Option<String>,
    pub last_human_request_id: Option<String>,
    pub last_human_line: Option<u64>,
    pub revision: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransitionOutcome {
    Applied,
    Duplicate { line_number: Option<u64> },
}
```

Implement `validate_quick_session_id`, `validate_quick_session_attempt_id`, `validate_quick_session_title`, `validate_quick_session_summary`, `truncate_quick_session_preview`, and `apply_quick_session_transition`. The transition enum must cover `HumanMessage`, `Claim`, `SetTitle`, `SetSummary`, `AgentReply`, `MarkError`, `Archive`, and `Unarchive`. Every applied mutation increments `revision`; duplicate operations do not.

Extend `LinkKind` with:

```rust
QuickSession {
    session_id: String,
    line_number: Option<u64>,
},
```

Add `QuickSessionListItem`, `QuickSessionDetail`, and response DTOs to `responses.rs` so daemon, runtime, and client share wire names.

- [ ] **Step 4: Expose validation/transitions through wasm and regenerate the package**

Add bindings with JSON inputs/outputs:

```rust
#[wasm_bindgen(js_name = validateQuickSessionId)]
pub fn validate_quick_session_id_wasm(id: &str) -> Result<(), JsValue>;

#[wasm_bindgen(js_name = parseQuickSessionMeta)]
pub fn parse_quick_session_meta_wasm(yaml: &str) -> Result<JsValue, JsValue>;

#[wasm_bindgen(js_name = applyQuickSessionTransition)]
pub fn apply_quick_session_transition_wasm(
    meta: JsValue,
    transition: JsValue,
) -> Result<JsValue, JsValue>;
```

Run:

```bash
npm --prefix products/gitim/frontend run build:wasm
cargo test -p gitim-core --locked
```

Expected: all core tests pass and wasm package generation exits 0.

- [ ] **Step 5: Commit Task 1**

```bash
git add crates/gitim-core crates/gitim-wasm
git commit -m "feat(core): define quick session protocol" \
  -m "Test: cargo test -p gitim-core --locked" \
  -m "Test: npm --prefix products/gitim/frontend run build:wasm" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 2: Native daemon object lifecycle and poll routing

**Files:**

- Create: `crates/gitim-daemon/src/quick_session_handlers.rs`
- Create: `crates/gitim-daemon/tests/quick_session_test.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Modify: `crates/gitim-daemon/src/handlers/poll.rs`
- Modify: `crates/gitim-daemon/src/handlers/serde.rs`

- [ ] **Step 1: Write failing daemon integration tests**

Cover real temp Git repositories and daemon handler calls for:

```rust
assert_create_is_one_commit_with_meta_and_thread().await;
assert_create_retry_returns_same_session().await;
assert_collision_with_different_immutable_fields_is_rejected().await;
assert_unknown_or_departed_agent_is_rejected().await;
assert_creator_send_request_id_is_idempotent().await;
assert_claim_is_agent_only_and_compare_and_set().await;
assert_agent_reply_requires_title_attempt_and_reply_line().await;
assert_agent_reply_retry_returns_original_line().await;
assert_stale_attempt_cannot_title_summarize_or_reply().await;
assert_mark_error_and_new_human_input_recover().await;
assert_archive_rolls_back_when_commit_fails().await;
assert_unarchive_restores_archived_from().await;
assert_poll_emits_scoped_quick_session_change().await;
```

- [ ] **Step 2: Run daemon tests and verify RED**

Run:

```bash
cargo test -p gitim-daemon --test quick_session_test --locked
```

Expected: compilation fails because request variants and handlers are absent.

- [ ] **Step 3: Implement daemon requests and handlers**

Add request variants with serde names matching the API contract. The central send request must carry both human and agent fields:

```rust
SendQuickSessionMessage {
    session_id: String,
    body: String,
    #[serde(default)]
    reply_to: Option<u64>,
    #[serde(default)]
    request_id: Option<String>,
    #[serde(default)]
    attempt_id: Option<String>,
    #[serde(default)]
    author: Option<String>,
}
```

In `quick_session_handlers.rs`, define validated path helpers, active/archive lookup, YAML load/write, canonical thread append, and mutation rollback. Each write must:

```text
validate input -> resolve actor -> acquire commit_lock -> re-read meta/thread
-> apply core transition -> write files -> commit -> rollback on commit failure
-> push daemon event only after success
```

List supports `archived`, `agent_id`, `actionable`, and `limit`. `actionable=true` reads the final real thread message and returns only `needs_title|active` sessions whose final author is `created_by`.

- [ ] **Step 4: Extend poll routing**

Classify:

```text
quick-sessions/<id>/session.meta.yaml      kind=quick_session_meta
quick-sessions/<id>/discussion.thread      kind=quick_session_thread
archive/quick-sessions/<id>/session.meta.yaml kind=quick_session_meta
archive/quick-sessions/<id>/discussion.thread kind=quick_session_thread
```

Thread entries carry `recipients: [meta.agent_id]`. Meta changes carry a synthetic entry with `session_id`, `agent_id`, `status`, and `revision`. Invalid or missing metadata logs a warning and emits no agent recipient.

Run:

```bash
cargo test -p gitim-daemon --test quick_session_test --locked
cargo test -p gitim-daemon --lib --locked
```

Expected: all daemon Quick Session and existing unit tests pass.

- [ ] **Step 5: Commit Task 2**

```bash
git add crates/gitim-daemon
git commit -m "feat(daemon): persist quick sessions" \
  -m "Test: cargo test -p gitim-daemon --test quick_session_test --locked" \
  -m "Test: cargo test -p gitim-daemon --lib --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 3: Client and agent-facing CLI

**Files:**

- Modify: `crates/gitim-client/src/client.rs`
- Create: `crates/gitim-cli/src/commands/session.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Modify: `crates/gitim-cli/src/main.rs`
- Modify: `crates/gitim-agent-provider/src/prompts.rs`
- Create: `crates/gitim-cli/tests/quick_session_cli.rs`

- [ ] **Step 1: Write failing client and CLI tests**

Assert Clap accepts:

```text
gitim session list --agent alice --actionable
gitim session read qs-01JZZZZZZZZZZZZZZZZZZZZZZZ
gitim session title qs-01JZZZZZZZZZZZZZZZZZZZZZZZ "Investigate auth" --attempt-id qa-01JZZZZZZZZZZZZZZZZZZZZZZZ
gitim session send qs-01JZZZZZZZZZZZZZZZZZZZZZZZ --stdin --reply-to 1 --attempt-id qa-01JZZZZZZZZZZZZZZZZZZZZZZZ
gitim session summarize qs-01JZZZZZZZZZZZZZZZZZZZZZZZ --stdin --attempt-id qa-01JZZZZZZZZZZZZZZZZZZZZZZZ
```

Client tests assert exact JSON method names and snake_case parameters.

- [ ] **Step 2: Run CLI tests and verify RED**

Run:

```bash
cargo test -p gitim-cli --test quick_session_cli --locked
cargo test -p gitim-client --locked
```

Expected: `session` is not a recognized subcommand and client methods are missing.

- [ ] **Step 3: Implement client methods and the `session` command group**

Add typed convenience methods for create/list/read/send/title/summary/claim/error/archive/unarchive. CLI exposes list/read/title/send/summarize; runtime-only claim/error remain client methods.

`session send` requires `--reply-to` and `--attempt-id` when invoked by an agent turn. It prints the returned `session_id`, `line_number`, `revision`, and stable ref in human or JSON output.

- [ ] **Step 4: Extend the default agent tool prompt**

Add a Quick Session section stating:

```text
When the runtime prompt says this is a Quick Session turn, use only the supplied
gitim session commands. Pass the exact session id, attempt id, and input line from
the prompt. Set the title before the first reply. The daemon rejects stale attempts.
```

Run:

```bash
cargo test -p gitim-cli --test quick_session_cli --locked
cargo test -p gitim-client --locked
cargo test -p gitim-agent-provider prompts --locked
```

Expected: all scoped tests pass.

- [ ] **Step 5: Commit Task 3**

```bash
git add crates/gitim-client crates/gitim-cli crates/gitim-agent-provider
git commit -m "feat(cli): add quick session tools" \
  -m "Test: cargo test -p gitim-cli --test quick_session_cli --locked" \
  -m "Test: cargo test -p gitim-client --locked" \
  -m "Test: cargo test -p gitim-agent-provider prompts --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 4: Runtime state, HTTP surface, and scoped events

**Files:**

- Create: `crates/gitim-runtime/src/quick_session_state.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`
- Modify: `crates/gitim-runtime/src/http.rs`
- Modify: `crates/gitim-runtime/src/fleet.rs`
- Create: `crates/gitim-runtime/tests/quick_session_http.rs`
- Modify: `crates/gitim-runtime/tests/fleet_http.rs`

- [ ] **Step 1: Write failing state and HTTP tests**

Test atomic state save/load, `0600`, corrupt JSON recovery, independent files, context-generation increments, route request/response mapping, departed/guest errors, and fleet event preservation.

The state shape is:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct QuickSessionRuntimeState {
    pub session_token: Option<String>,
    pub session_usage: Option<SessionUsageSnapshot>,
    pub estimated_tokens: u64,
    pub last_session_usage: Option<LastSessionUsage>,
    pub reset_required: bool,
    pub last_attempted_line: Option<u64>,
    pub last_completed_input_line: Option<u64>,
    pub last_completed_line: Option<u64>,
    pub context_generation: u64,
}
```

- [ ] **Step 2: Run runtime tests and verify RED**

Run:

```bash
cargo test -p gitim-runtime --test quick_session_http --locked
cargo test -p gitim-runtime quick_session_state --locked
```

Expected: module, event fields, and routes are absent.

- [ ] **Step 3: Implement private state persistence and event schema**

State path is `<workspace>/.gitim-runtime/quick-sessions/<id>.state.json`. Save with `NamedTempFile::persist`, create the directory, and chmod the final file to `0600` on Unix. Corrupt state is renamed with a `.corrupt-<timestamp>` suffix and returns default state while preserving the durable transcript.

Extend `AgentActivityEvent` with optional/additive fields:

```rust
#[serde(default, skip_serializing_if = "ActivityScope::is_main")]
pub scope: ActivityScope,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub session_id: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub r#ref: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub session_revision: Option<u64>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub attempt_id: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub context_generation: Option<u64>,
```

Update every existing constructor with main-scope defaults and preserve the fields through fleet serialization.

- [ ] **Step 4: Implement runtime HTTP handlers**

Add routes under the workspace router for create/list/read/send/archive/unarchive. Each resolves `human_repo_path`, calls the typed `GitimClient`, preserves daemon `error_code`, and returns non-2xx only through the existing `api_response_to_json` convention. Create verifies the chosen handler exists in active users; local runtime liveness is informational.

Run:

```bash
cargo test -p gitim-runtime --test quick_session_http --locked
cargo test -p gitim-runtime --test fleet_http --locked
cargo test -p gitim-runtime --lib --locked
```

Expected: all scoped runtime tests pass.

- [ ] **Step 5: Commit Task 4**

```bash
git add crates/gitim-runtime
git commit -m "feat(runtime): expose quick session state and api" \
  -m "Test: cargo test -p gitim-runtime --test quick_session_http --locked" \
  -m "Test: cargo test -p gitim-runtime --test fleet_http --locked" \
  -m "Test: cargo test -p gitim-runtime --lib --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 5: Sequential Quick Session execution inside AgentLoop

**Files:**

- Create: `crates/gitim-runtime/src/quick_session_executor.rs`
- Modify: `crates/gitim-runtime/src/agent_loop.rs`
- Modify: `crates/gitim-runtime/src/http.rs`
- Create: `crates/gitim-runtime/tests/quick_session_executor.rs`
- Modify: `crates/gitim-runtime/tests/agent_loop.rs`

- [ ] **Step 1: Write failing scheduler and executor tests**

Use the mock provider and temp daemon repos to prove:

```rust
quick_changes_are_not_formatted_into_primary_prompt();
quick_ids_enqueue_once_in_fifo_order();
primary_turn_runs_before_one_quick_turn();
startup_scan_finds_pre_cursor_actionable_session();
stale_running_claim_becomes_error_without_execution();
human_input_during_running_remains_actionable_after_reply();
archive_during_running_rejects_late_completion();
provider_turn_inherits_config_but_not_primary_token();
title_and_reply_complete_the_claim();
missing_title_or_reply_marks_error_once();
late_attempt_or_generation_is_discarded();
usage_and_reset_change_only_quick_state();
self_managed_provider_skips_runtime_reset();
```

- [ ] **Step 2: Run executor tests and verify RED**

Run:

```bash
cargo test -p gitim-runtime --test quick_session_executor --locked
cargo test -p gitim-runtime --test agent_loop quick_session --locked
```

Expected: queue and executor APIs are missing.

- [ ] **Step 3: Implement queue scheduling in AgentLoop**

Add retained provider env and these fields:

```rust
quick_session_queue: VecDeque<String>,
quick_session_queued: HashSet<String>,
quick_session_last_scan: Option<Instant>,
```

On each poll, extract `quick_session_meta|quick_session_thread` changes whose recipients include `self.handler`, enqueue their session ids, and exclude them from `format_changes_as_prompt`. After the primary turn attempt, call `run_next_quick_session`; it processes at most one id. `init` performs an actionable scan, and recovery scan cadence is at least 60 seconds.

- [ ] **Step 4: Implement QuickSessionExecutor**

Executor flow:

```text
read detail -> select creator input line -> generate attempt id -> daemon claim
-> load local state -> create fresh provider object from retained config
-> build cold/resume options with Quick Session system prompt
-> stream scoped events and detect RESET -> await result
-> re-read detail -> verify title/reply/attempt/revision
-> persist token, usage, estimate, completion line
-> mark error on incomplete/failed turn
```

The prompt includes exact session id, attempt id, claimed input line, stable ref, summary, and bounded transcript. Scoped events include the claim revision and captured context generation. Before applying completion, compare both the daemon attempt id and local generation.

Run:

```bash
cargo test -p gitim-runtime --test quick_session_executor --locked
cargo test -p gitim-runtime --test agent_loop --locked
```

Expected: all executor and existing agent-loop tests pass.

- [ ] **Step 5: Commit Task 5**

```bash
git add crates/gitim-runtime
git commit -m "feat(runtime): execute isolated quick sessions" \
  -m "Test: cargo test -p gitim-runtime --test quick_session_executor --locked" \
  -m "Test: cargo test -p gitim-runtime --test agent_loop --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 6: Browser daemon and backend parity

**Files:**

- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`
- Create: `products/gitim/frontend/src/daemon-web/quick-session-parity.test.ts`
- Modify: `products/gitim/frontend/src/lib/backend.ts`
- Modify: `products/gitim/frontend/src/lib/backend.test.ts`

- [ ] **Step 1: Write failing browser parity tests**

Cover client-generated idempotent create, handler validation, canonical first thread line, human send request-id dedupe, active/archive reads, archive restore, epoch fence, reconnect commit-only response, poll classification, and golden YAML equality with Rust fixtures.

- [ ] **Step 2: Run Vitest and verify RED**

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/daemon-web/quick-session-parity.test.ts \
  src/lib/backend.test.ts
```

Expected: worker methods and backend interface are missing.

- [ ] **Step 3: Implement browser handlers with wasm transitions**

Add create/list/read/send/archive/unarchive functions. Every mutation runs inside `withRepoLock`, checks `assertNotRedirected`, calls `validateQuickSessionId` plus `applyQuickSessionTransition`, formats thread lines through wasm, commits the same relative paths as native, and calls `syncAfterCommit`.

The worker RPC and `LocalBackend` expose:

```ts
export interface QuickSessionBackend {
  createQuickSession(input: CreateQuickSessionInput): Promise<ApiResponse>;
  listQuickSessions(query?: QuickSessionListQuery): Promise<ApiResponse>;
  readQuickSession(id: string): Promise<ApiResponse>;
  sendQuickSessionMessage(id: string, input: SendQuickSessionInput): Promise<ApiResponse>;
  archiveQuickSession(id: string): Promise<ApiResponse>;
  unarchiveQuickSession(id: string): Promise<ApiResponse>;
}
```

- [ ] **Step 4: Verify browser parity**

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/daemon-web/quick-session-parity.test.ts \
  src/daemon-web/handlers.test.ts \
  src/lib/backend.test.ts
```

Expected: all selected tests pass.

- [ ] **Step 5: Commit Task 6**

```bash
git add products/gitim/frontend/src/daemon-web products/gitim/frontend/src/lib/backend.ts products/gitim/frontend/src/lib/backend.test.ts
git commit -m "feat(frontend): add browser quick session parity" \
  -m "Test: npm --prefix products/gitim/frontend exec vitest -- run src/daemon-web/quick-session-parity.test.ts src/daemon-web/handlers.test.ts src/lib/backend.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 7: Frontend data model, store, and scoped activity

**Files:**

- Modify: `products/gitim/frontend/src/lib/types.ts`
- Modify: `products/gitim/frontend/src/lib/client.ts`
- Create: `products/gitim/frontend/src/lib/quick-session-ref.ts`
- Create: `products/gitim/frontend/src/lib/quick-session-ref.test.ts`
- Create: `products/gitim/frontend/src/hooks/use-quick-session-store.ts`
- Create: `products/gitim/frontend/src/hooks/use-quick-session-store.test.ts`
- Modify: `products/gitim/frontend/src/hooks/use-agent-activity.ts`
- Modify: `products/gitim/frontend/src/hooks/use-agent-activity.test.ts`
- Modify: `products/gitim/frontend/src/hooks/use-poll-loop.ts`

- [ ] **Step 1: Write failing store, ref, and event tests**

Assert type mapping, create/open/send/archive state, stale response cancellation, workspace reset, revision/generation filtering, main usage isolation, session event routing, poll-triggered refresh, ref parsing boundaries, and ULID generation.

- [ ] **Step 2: Run frontend data tests and verify RED**

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/hooks/use-quick-session-store.test.ts \
  src/hooks/use-agent-activity.test.ts \
  src/lib/quick-session-ref.test.ts
```

Expected: store and ref modules are missing.

- [ ] **Step 3: Implement types, HTTP methods, ref utility, and store**

The store keeps `items`, `selectedId`, `detailById`, `runtimeById`, `showArchived`, `loading`, and per-operation errors. Every async action captures `activeSlug`; results are discarded if the workspace changed. Session runtime overlays carry status, revision, generation, latest event, and usage.

`client.ts` delegates to `QuickSessionBackend` in browser mode and native workspace routes in HTTP mode. Client-generated session ids and human request ids use cryptographically random ULIDs.

- [ ] **Step 4: Route poll and SSE events**

`useAgentActivitySSE` handles `scope === "quick_session"` before main usage/activity logic. The Quick Session store orders metadata events by `session_revision`. It accepts a streaming event only when `attempt_id` matches the item's active claim and `context_generation` is current; title updates can increment metadata revision without suppressing progress from the same attempt. `use-poll-loop` refreshes list/detail only when a Quick Session change appears; there is no dedicated interval.

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/hooks/use-quick-session-store.test.ts \
  src/hooks/use-agent-activity.test.ts \
  src/lib/quick-session-ref.test.ts
```

Expected: all selected tests pass.

- [ ] **Step 5: Commit Task 7**

```bash
git add products/gitim/frontend/src/lib products/gitim/frontend/src/hooks
git commit -m "feat(frontend): manage quick session state" \
  -m "Test: npm --prefix products/gitim/frontend exec vitest -- run src/hooks/use-quick-session-store.test.ts src/hooks/use-agent-activity.test.ts src/lib/quick-session-ref.test.ts" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 8: Conversation hub, composer drop, and reference previews

**Files:**

- Create: `products/gitim/frontend/src/components/sessions/quick-session-hub.tsx`
- Create: `products/gitim/frontend/src/components/sessions/quick-session-list.tsx`
- Create: `products/gitim/frontend/src/components/sessions/quick-session-panel.tsx`
- Create: `products/gitim/frontend/src/components/sessions/quick-session-hub.test.tsx`
- Modify: `products/gitim/frontend/src/components/layout/app-shell.tsx`
- Modify: `products/gitim/frontend/src/components/chat/input-area.tsx`
- Modify: `products/gitim/frontend/src/components/chat/input-area.test.tsx`
- Modify: `products/gitim/frontend/src/components/chat/message-body.tsx`
- Modify: `products/gitim/frontend/src/components/chat/message-body.test.tsx`
- Modify: `products/gitim/frontend/src/components/chat/reference-preview.tsx`
- Modify: `products/gitim/frontend/src/components/chat/reference-preview.test.tsx`
- Create: `products/gitim/frontend/e2e/quick-sessions.spec.ts`

- [ ] **Step 1: Write failing component and E2E tests**

Component tests cover hover open/leave delay, click pin, keyboard focus, empty/loading/error states, agent selection, create, detail selection, send, archive filter, copy, draggable payload, drop insertion into matching channel/card drafts, stale-scope rejection, active/archived ref previews, and optional line highlighting.

The Playwright test creates a session through the mocked runtime fixture, observes the title/reply refresh, drags the ref into a channel composer, verifies no automatic send, then archives and reveals it with the archived filter.

- [ ] **Step 2: Run UI tests and verify RED**

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/components/sessions/quick-session-hub.test.tsx \
  src/components/chat/input-area.test.tsx \
  src/components/chat/message-body.test.tsx \
  src/components/chat/reference-preview.test.tsx
```

Expected: Quick Session components and reference fragment are absent.

- [ ] **Step 3: Implement the hub and draft insertion**

Use the existing `Popover`, `Button`, agent/store hooks, and `DESIGN.md` tokens. The top-bar trigger is visible on desktop and accessible by click/focus. Pointer leave closes after 180 ms unless pinned. The three components keep list, create form, and conversation responsibilities separate.

Rows expose `draggable` data with MIME `application/x-gitim-quick-session-ref`. `InputArea` accepts drops only when payload workspace key matches its current workspace and inserts the stable ref at the cursor without calling `onSend`.

- [ ] **Step 4: Implement message rendering and preview**

`MessageBody` renders `session:<id>` and optional line targets with `QuickSessionReferenceLink`. The preview calls `readQuickSession`, handles archived lookup through the same endpoint, shows title/agent/status/summary/latest preview, and renders a bounded transcript window with the requested line highlighted.

Run:

```bash
npm --prefix products/gitim/frontend exec vitest -- run \
  src/components/sessions/quick-session-hub.test.tsx \
  src/components/chat/input-area.test.tsx \
  src/components/chat/message-body.test.tsx \
  src/components/chat/reference-preview.test.tsx
npm --prefix products/gitim/frontend run build
```

Expected: selected UI tests and production build pass.

- [ ] **Step 5: Commit Task 8**

```bash
git add products/gitim/frontend/src products/gitim/frontend/e2e/quick-sessions.spec.ts
git commit -m "feat(frontend): add quick session hub" \
  -m "Test: npm --prefix products/gitim/frontend exec vitest -- run src/components/sessions/quick-session-hub.test.tsx src/components/chat/input-area.test.tsx src/components/chat/message-body.test.tsx src/components/chat/reference-preview.test.tsx" \
  -m "Test: npm --prefix products/gitim/frontend run build" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 9: Cross-layer verification and release-ready cleanup

**Files:**

- Modify as required by failures in Tasks 1-8
- Modify: `docs/plans/quick-sessions/00-requirements.md`
- Modify: `docs/plans/quick-sessions/01-eng-review.md`
- Modify: `docs/plans/quick-sessions/02-implementation-plan.md`

- [ ] **Step 1: Run the scoped cross-layer test matrix**

```bash
cargo test -p gitim-core --locked
cargo test -p gitim-daemon --test quick_session_test --locked
cargo test -p gitim-daemon --lib --locked
cargo test -p gitim-client --locked
cargo test -p gitim-cli --test quick_session_cli --locked
cargo test -p gitim-agent-provider prompts --locked
cargo test -p gitim-runtime --test quick_session_http --locked
cargo test -p gitim-runtime --test quick_session_executor --locked
cargo test -p gitim-runtime --test agent_loop --locked
npm --prefix products/gitim/frontend test
npm --prefix products/gitim/frontend run lint
npm --prefix products/gitim/frontend run build
cargo fmt --all -- --check
git diff --check
```

Expected: every command exits 0. Do not run workspace-wide `cargo test` unless shared protocol/build changes cause a coverage uncertainty that the scoped matrix cannot answer.

- [ ] **Step 2: Run the Quick Session E2E when its fixture is available**

```bash
npm --prefix products/gitim/frontend exec playwright test e2e/quick-sessions.spec.ts
```

Expected: the complete create/title/reply/ref/archive flow passes. If the repository's E2E fixture cannot launch in the current environment, record the exact environment error in the PR test plan and keep all unit/integration layers green.

- [ ] **Step 3: Audit the final diff against every acceptance criterion**

Check each item in `00-requirements.md` against a code path and test. Confirm wasm package changes are committed, no demo/seed behavior remains, no secret or runtime state file is tracked, and every new write path has rollback or atomic persistence.

- [ ] **Step 4: Run format/clippy hook-equivalent verification and commit cleanup**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked
git diff --check
```

Commit only if Task 9 introduced changes:

```bash
git add docs/plans/quick-sessions crates products/gitim/frontend
git commit -m "fix(quick-sessions): close integration gaps" \
  -m "Test: scoped Quick Sessions matrix documented in docs/plans/quick-sessions/02-implementation-plan.md" \
  -m "Test: cargo fmt --all -- --check" \
  -m "Test: cargo clippy --workspace --all-targets --no-deps --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

- [ ] **Step 5: Hand off to two-round review**

Record the base and head commits:

```bash
git merge-base HEAD origin/main
git rev-parse HEAD
git status --short --branch
```

Expected: named feature branch, clean worktree, and all implementation commits ahead of `origin/main`.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope and strategy | 0 | Not run | PR #47 product semantics were supplied by the user and independently specified |
| Codex Review | `/codex review` | Independent second opinion | 3 | Pass | 3 P1 and 1 P2 findings fixed; final rerun approved |
| Eng Review | `/plan-eng-review` | Architecture and tests | 1 | Clear | 8 findings resolved before implementation |
| Design Review | Task 8 spec and quality reviews | UI and UX correctness | 2 | Approved | Radix focus, workspace scope, bounded reads, stale async state, and reference DnD verified |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | Not required | Existing CLI, HTTP, Git, and browser conventions reused |

**CROSS-MODEL:** Independent reviews converged on durable identity, crash-safe turn claims, archive-wins convergence, transactional conflict replay, provider cancellation, shared protocol rules, and stale-event fencing.

**UNRESOLVED:** 0

**VERDICT:** Implementation and review gates passed; ready for draft PR comparison and merge review.
