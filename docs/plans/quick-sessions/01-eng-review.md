# Quick Sessions Engineering Review

Status: Cleared for planning
Date: 2026-07-11
Source: `00-requirements.md`

## Scope challenge

This feature crosses the protocol, native daemon, runtime, CLI, browser daemon, HTTP client, and WebUI. The breadth is essential because a partially implemented object would break GitIM's cross-node and browser/runtime equivalence. The implementation stays bounded by reusing the canonical thread protocol, daemon commit machinery, agent provider factory, runtime activity stream, frontend backend abstraction, and existing reference-preview patterns.

The core scope is accepted with these constraints:

- One agent owns each session for its full lifetime.
- The existing per-agent loop is the only provider scheduler.
- Git carries metadata and transcript; runtime-local state never syncs.
- The browser implements durable user operations and never executes providers.
- Quick Sessions do not enter boards, card workflow, or channel routing.

The feature will touch more than eight files because each existing public surface has a contract to extend. The work is divided into independently testable protocol, daemon, runtime, browser, and UI layers instead of introducing a parallel service.

## What already exists

| Need | Existing capability | Reuse decision |
|------|---------------------|----------------|
| Durable conversation text | `gitim-core` formatter/parser and `.thread` validation | Reuse unchanged for `discussion.thread` |
| Atomic native writes | daemon `commit_lock`, `GitStorage::commit`, rollback patterns in card/archive handlers | Reuse for every session mutation |
| Cross-node delivery | daemon sync loop and poll diff | Extend poll classification with a Quick Session kind |
| Agent execution | one `AgentLoop` per agent and provider factory | Extend the existing loop with a sequential Quick Session queue |
| Agent tools | `gitim` CLI over daemon IPC | Add a `session` command group |
| Runtime events | workspace-scoped `AgentActivityEvent` broadcast and fleet relay | Add backward-compatible scope and session fields |
| Browser writes | daemon-web repo lock, epoch fence, wasm formatter/parser, sync-after-commit | Add equivalent Quick Session handlers |
| Frontend transport | `Backend`, `HttpBackend`, and `LocalBackend` | Add one Quick Session backend interface |
| Reference previews | message parser and hover preview components | Add a Quick Session reference fragment and preview |
| Scoped drafts | `InputArea` workspace/scope identity | Use the same identity to validate drag/drop insertion |

## Architecture decisions

### 1. One scheduler per agent

`AgentLoop` owns a deduplicated FIFO of Quick Session ids. Poll changes enqueue ids addressed to that agent. Startup and throttled recovery scans enqueue actionable sessions that predate the cursor. The runtime processes primary input first and at most one Quick Session afterward.

```text
agent clone daemon poll
        |
        v
+------------------------+
| split poll changes     |
|                        |
| normal IM   quick ids  |
+-----+----------+-------+
      |          |
      v          v
 main provider   FIFO (dedupe)
 turn            |
                 v
          one quick turn
                 |
                 v
          next poll cycle
```

This keeps provider-profile access sequential without a second poller, background loop, or shared execution lock. Quick Session execution cannot mutate the primary provider instance or primary `AgentState`.

### 2. Durable state machine with claimed running state

`running` is a Git-synced claim on one input line. The runtime writes the claim before provider execution so crash recovery never guesses whether an input already produced external side effects.

```text
create
  |
  v
needs_title / active --claim(input line)--> running
       ^                                      |
       |                           title + agent reply
       |                                      |
       +---------------- active <--------------+
                                              |
                                      failure / stale claim
                                              v
                                            error
                                              |
                                    new human input
                                              |
                                     title already set?
                                      | yes      | no
                                      v          v
                                    active   needs_title

needs_title / running / active / error --archive--> archived
archived --unarchive--> needs_title | active | error
```

The archive operation stores the prior stable status in metadata. Archiving a running session clears the claim and stores the title-derived stable status, so late provider writes fail and unarchive never resurrects a phantom running turn. An agent reply is valid only after title creation and must point to the claimed input line. A human message in `error` clears the error and re-enters the correct title-derived state.

### 3. Actionability is derived from the transcript

A session is actionable when a creator-authored line is newer than `last_completed_input_line` and its status is `needs_title` or `active`. The runtime claims the highest current creator line as the inclusive input-batch boundary. The daemon rejects a second claim, validates the agent reply against the claim, and marks an incomplete or stale claim as `error`; creator messages appended during a running turn remain queued for the next claim.

Transcript-derived actionability survives runtime-state loss and cross-node sync. The durable claim prevents concurrent or crash-induced duplicate execution, and local line markers preserve context continuity.

### 4. Agent-visible title gate

The selected provider receives a Quick Session system prompt that exposes `gitim session title`, `send`, and `summarize`. The daemon enforces title-before-reply. Runtime verification reads the durable object after provider completion and requires:

- a title for the first turn;
- an agent-authored line after the attempted creator line;
- a summary update before accepting a requested context reset.

This makes the title an agent action observable in the protocol instead of a hidden runtime-generated label.

### 5. Scoped events are additive

`AgentActivityEvent` gains:

- `scope`, defaulting to `agent_main` when absent;
- `session_id`, present for `quick_session`;
- `ref`, present for `quick_session`.
- `session_revision`, present for `quick_session`;
- `attempt_id`, present for a running Quick Session turn;
- `context_generation`, present for `quick_session` provider events.

Fleet relays preserve these fields without changing their envelope. Frontend handlers route the event before applying usage or activity updates. Metadata events use revision ordering; streaming events require the current durable attempt id and context generation, so a title revision during the same attempt does not hide progress.

### 6. Browser parity is a storage concern

The browser worker creates and mutates the same files and commit units as the native daemon. It uses the repo lock, epoch fence, canonical wasm thread formatter/parser, wasm-exported Quick Session transitions, and sync-after-commit behavior. Runtime-only title, summary, and error transitions arrive through Git after execution on the agent node.

### 7. Revisioned, idempotent mutations

Every metadata mutation increments `revision`. The client-generated session id makes create idempotent; human-send requests carry request ids. Provider claims carry attempt ids, and title/summary/reply operations compare against the active attempt. The first accepted agent reply records `last_completed_attempt_id`; a retry returns the original committed line instead of appending a duplicate.

Quick Session local state also carries `context_generation`. Reset increments it, and executor results/events captured under an older generation are discarded. These two counters separate durable Git object ordering from local provider-context ordering.

## Data flow

```text
Human node                              Agent-owning node
-----------                             -----------------
WebUI / browser hub
  | create(agent, message)
  v
daemon writer
  | one commit: meta + thread
  v
Git remote  --------------------------> sync loop
                                            |
                                            v
                                      daemon poll
                                            |
                                      enqueue session id
                                            |
                                      AgentLoop quick turn
                                            |
                               gitim session title/send
                                            |
                                      daemon commits
                                            v
Git remote  <-------------------------- sync loop
  |
  v
poll refresh / ref preview / hub
```

## Code quality review

1. Quick Session metadata, validation, response DTOs, and link kind belong in `gitim-core`; native and browser code consume one schema vocabulary.
2. Native daemon handlers live in one focused module with shared path, load, write, permission, and commit helpers.
3. Runtime persistence and execution are separate modules: state handles atomic private files; executor handles prompt, provider, events, and post-turn verification.
4. `AgentLoop` only owns queue scheduling and configuration handoff. Provider-turn behavior stays in the Quick Session executor.
5. Frontend transport mapping stays in `client.ts` and `backend.ts`; the Zustand store owns async UI state; components remain presentational.
6. Session ref parsing has one frontend utility used by message rendering, drag/drop, and previews.
7. Protocol comments explain state-machine and scheduler boundaries. UI comments are limited to non-obvious scope routing.

## Test review

```text
CODE PATHS                                           USER FLOWS
[+] core id/meta/ref                                 [+] Create and first turn
  +-- valid ULID / invalid boundary                    +-- create success [unit + integration]
  +-- serde legacy/default fields                      +-- invalid agent/message [integration]
  +-- active/archive path derivation                    +-- title then reply [integration + mock provider]
  +-- optional :L line parsing                          +-- reply before title rejected [integration]

[+] daemon mutations                                 [+] Continue and recover
  +-- create collision retry                            +-- follow-up message resumes same provider token
  +-- permission gates                                  +-- incomplete turn becomes recoverable error
  +-- title gate                                        +-- new human input retries once
  +-- commit failure rollback                           +-- restart preserves session token
  +-- archive/unarchive restore                         +-- startup scan finds old actionable session

[+] runtime scheduler                                [+] Context handoff
  +-- poll split excludes quick input from main          +-- usage warning arms reset
  +-- FIFO dedupe and one-per-cycle                      +-- summary + RESET clears only quick token
  +-- primary-before-quick ordering                      +-- missing summary produces visible error
  +-- scoped activity/usage routing                      +-- self-managed provider skips runtime reset

[+] browser daemon                                  [+] Hub and references
  +-- create/read/send/archive parity                    +-- hover/focus/pin accessibility [component]
  +-- epoch fence and reconnect error                     +-- active/archive filtering [component]
  +-- repo-lock serialization                            +-- send and error recovery [component]
  +-- poll classification                                +-- scoped activity isolation [unit]
                                                        +-- drag into channel/card draft [component]
[+] HTTP/backend/client                                  +-- ref preview active/archived/line [component]
  +-- native route status mapping                        +-- end-to-end create to rendered reply [E2E]
  +-- local worker RPC mapping
  +-- abort/stale workspace handling
```

The implementation plan must use TDD for each box. Mock-provider integration verifies the prompt/tool contract and session-token isolation. No live provider test is required in the default suite.

### Failure modes

| Code path | Production failure | Test | Handling | User signal |
|-----------|--------------------|------|----------|-------------|
| Create | id collision or partial filesystem write | collision + rollback integration | retry, rollback, typed error | inline create error |
| Cross-node sync | owning node offline | actionable startup test | durable waiting state | waiting status |
| Turn claim | runtime crashes after external side effect | stale-claim integration | mark error without automatic replay | interrupted-turn error |
| Concurrent human input | user sends while agent is running | input-boundary integration | leave newer input queued after current reply | next turn begins normally |
| Running archive | user archives during provider execution | stale-attempt integration | clear claim and reject late writes | archived state remains stable |
| Scheduler | quick input leaks into main prompt | poll split regression | kind partition + recipient check | no silent cross-context leak |
| Provider turn | provider returns without title/reply | mock-provider integration | mark durable error, stop retry | error row with retry hint |
| Late completion | old provider turn finishes after a newer claim/reset | generation + attempt tests | compare-and-set rejection | current session state unchanged |
| Client retry | create/send response is lost | idempotency integration | return prior object/line | request succeeds once |
| Title/send CLI | wrong agent or creator | daemon permission integration | typed denial | CLI/API error |
| Runtime state | truncated JSON or failed atomic replace | state unit tests | quarantine/reset token, keep transcript | recoverable error event |
| Context reset | reset emitted without summary | executor integration | reject reset, mark error | summary-required message |
| Archive | commit fails after move | rollback integration | restore active directory/meta | archive error, item remains |
| Browser write | epoch redirected or token revoked | worker tests | fence or commit-only response | reload/reconnect prompt |
| Event stream | event reaches wrong session | store routing tests | strict scope/session filter | no visible leakage |
| Drag/drop | stale workspace or composer target | component test | scope-key check | ref stays in source/copy fallback |
| Preview | archived/missing session | component test | archived lookup then not-found state | explicit unavailable preview |

No reviewed path is allowed to fail silently without both error handling and test coverage.

## Performance review

- Poll changes enqueue session ids in O(changed files), reusing the existing Git diff.
- Startup discovery scans session metadata once. A throttled recovery scan runs no more than once per minute and requests only sessions assigned to the current agent.
- The daemon list endpoint supports `agent_id`, `archived`, `actionable`, and `limit` filters and sorts by `updated_at`.
- The hub performs an initial list read, refreshes on existing IM poll changes, and applies scoped SSE events. It does not own an independent fixed-interval poller.
- Transcript reads are bounded for hub and preview views; provider cold start receives the durable summary plus a bounded recent window.
- Quick Session state writes are per-session files, avoiding a workspace-global lock.
- Revision, request-id, and attempt checks are O(1) comparisons on the target session metadata.

## Independent challenge resolution

The final contract includes durable attempt claims, idempotent create/send/reply behavior, revisioned events, local context generations, wasm-shared transition rules, and stale-claim recovery. Session identity remains the opaque ULID and is independent of title or runtime process identity. Creation remains one locked commit with rollback, so it has no partially provisioned provider process to reconcile.

## NOT in scope

- Agent reassignment: provider state and ownership remain stable for the session lifetime.
- Multi-agent sessions: shared work belongs in channels or flows.
- Hard delete: archive preserves addressable Git history and refs.
- Provider-state transfer between nodes: local provider tokens remain machine-bound.
- Global search indexing: direct refs, previews, and the hub provide retrieval in V1.
- Full mobile drag gesture: copy and composer paste remain available where HTML drag/drop is unavailable.

## Deferred TODO assessment

No existing `TODOS.md` item blocks Quick Sessions, and this review does not create a deferred item required for correctness. The global-search boundary can be reconsidered after Quick Session usage establishes a retrieval need.

## Implementation sequencing

| Lane | Modules | Depends on |
|------|---------|------------|
| A | `gitim-core`, `gitim-wasm` | none |
| B | native daemon, client, CLI | A |
| C | runtime state/executor/scheduler/HTTP | B |
| D | browser daemon/backend | A |
| E | frontend store/components/reference UX | B and D transport contracts |
| F | cross-layer integration/E2E | C, D, E |

The shared worktree and public type boundaries make the safe execution order A, then B and D, then C and E, then F. Within the SOP implementation phase, code changes remain sequential because all agents share the same worktree; review agents can run independently after each committed task.

## Review result

- Scope: accepted with one scheduler, one-agent ownership, and explicit V1 boundaries.
- Architecture: seven decisions locked; no unresolved decision.
- Code quality: module boundaries and shared-source rules locked.
- Tests: all new code paths and user flows mapped; missing tests are implementation requirements.
- Performance: poll-driven refresh plus bounded recovery scans; no per-feature hot poller.
- Critical gaps: zero after applying the required error and test paths above.
- Lake score: complete protocol surface selected for the V1 product semantics.

## GSTACK REVIEW REPORT

| Review | Trigger | Why | Runs | Status | Findings |
|--------|---------|-----|------|--------|----------|
| CEO Review | `/plan-ceo-review` | Scope and strategy | 0 | Not run | Product semantics supplied by PR #47 and independently specified |
| Codex Review | `/codex review` | Independent second opinion | 1 | Fallback reviewer completed | Durable claims, idempotency, revisions, generations, and wasm parity added |
| Eng Review | `/plan-eng-review` | Architecture and tests | 1 | Clear | 8 findings resolved, 0 critical gaps |
| Design Review | `/plan-design-review` | UI and UX gaps | 0 | Pending implementation review | `DESIGN.md` constraints locked |
| DX Review | `/plan-devex-review` | Developer experience gaps | 0 | Not required | Existing CLI and backend conventions reused |

**CROSS-MODEL:** Both reviews converged on durable identity, crash-safe turn claims, idempotent replies, shared protocol rules, and stale-event fencing.

**UNRESOLVED:** 0

**VERDICT:** Engineering review cleared; ready for the TDD implementation plan.
