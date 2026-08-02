# Shared Skills Event Protocol Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:test-driven-development` for every behavior change. Execute tasks inline in order.

**Goal:** Add an explicit, Git-audited shared Skill layer whose state is reduced from immutable events and immutable package revisions.

**Architecture:** `gitim-core` owns portable validation and deterministic reduction. `gitim-daemon` materializes immutable files under `skills/`, applies permissions under the existing `commit_lock`, and creates normal Git commits; existing sync transports those commits. `gitim-client`, `gitim-cli`, and the provider prompt expose bounded discovery and just-in-time loading.

**Tech Stack:** Rust stable, serde YAML/JSON, SHA-256, ULID, Tokio Unix-socket IPC, clap, existing `GitStorage`.

## Global Constraints

- Runtime-native Skill directories remain isolated; catalog and package content are loaded explicitly through the GitIM CLI.
- Shared Skill commits use the existing GitIM commit and sync path.
- Every accepted non-idempotent shared write creates exactly one immutable event and one normal attributable Git commit.
- Event reduction must be deterministic and tolerate concurrent stale events as visible but ineffective history.
- Package loading is bounded and never executes package content.
- Keep new production code focused; split files by ID/ref, package, model/reducer, store, handler, and CLI responsibility.

---

### Task 1: Core IDs, references, and packages

**Files:**

- Create: `crates/gitim-core/src/skill/id.rs`
- Create: `crates/gitim-core/src/skill/reference.rs`
- Create: `crates/gitim-core/src/skill/package.rs`
- Create: `crates/gitim-core/src/skill/error.rs`
- Create: `crates/gitim-core/src/skill/mod.rs`
- Modify: `crates/gitim-core/src/lib.rs`
- Modify: `crates/gitim-core/Cargo.toml`
- Modify: `Cargo.toml`
- Test: `crates/gitim-core/tests/skill_protocol.rs`
- Test: `crates/gitim-core/tests/skill_package.rs`

**Interfaces:**

- Produces `SkillSlug`, `RevisionId`, `ProposalId`, `EventId`, `SkillReference`, `SkillError`.
- Produces `validate_package_entries`, `canonical_package_sha256`, `media_type_for_path`, and package limits.

- [x] Write failing tests for slug/ID canonicalization, pinned/unpinned refs, portable paths, frontmatter, symlink rejection, file/byte limits, resource descriptors, and stable hashes.
- [x] Run `cargo test -p gitim-core --test skill_protocol --test skill_package --locked` and verify failures are caused by the missing module.
- [x] Implement the minimal public types and validation. IDs serialize as strings; event generation accepts the greatest observed event ID and guarantees a greater result.
- [x] Re-run the two test targets and verify they pass.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 2: Pure event reducer

**Files:**

- Create: `crates/gitim-core/src/skill/model.rs`
- Create: `crates/gitim-core/src/skill/reducer.rs`
- Modify: `crates/gitim-core/src/skill/mod.rs`
- Test: `crates/gitim-core/tests/skill_reducer.rs`

**Interfaces:**

- Consumes IDs and `ResourceDescriptor` from Task 1.
- Produces `SkillRevisionMeta`, `SkillEvent`, `SkillEventKind`, `SkillState`, `SkillProposal`, `ProposalStatus`, `ReducedEvent`, and `reduce_skill`.

- [x] Write failing reducer tests for create, proposals, comments, publish/reject/withdraw, role and metadata permissions, archive semantics, competing publish tie-breaks, concurrent proposal convergence, invalid event order, and ineffective-event history.
- [x] Run `cargo test -p gitim-core --test skill_reducer --locked` and verify it fails because reducer types are absent.
- [x] Implement tagged serde event types and a single sorted reduction loop. Keep semantic precondition failures in `ReducedEvent { effective: false, reason }`; return `SkillError::InvalidHistory` only when the stream cannot identify one valid Skill.
- [x] Re-run reducer tests and the complete `gitim-core` crate tests.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 3: Daemon repository store

**Files:**

- Create: `crates/gitim-daemon/src/skill_store.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`
- Test: `crates/gitim-daemon/tests/skill_store.rs`

**Interfaces:**

- Consumes core package/reducer APIs and `AppState::commit_lock`/`GitStorage`.
- Produces `SkillStore` read methods for catalog, state, published/candidate load, resources, revisions, and history.
- Produces mutation methods for create, propose, comment, proposal transitions, metadata, roles, and archive state.

- [x] Write failing filesystem tests using temporary Git repositories for package import, event/revision path validation, catalog isolation of one invalid Skill, pinned archived loads, unpublished candidate protection, idempotent event replay, and commit-failure rollback.
- [x] Run `cargo test -p gitim-daemon --test skill_store --locked` and verify missing-store failures.
- [x] Implement safe recursive package collection, atomic YAML/file writes, deterministic reads, active-member checks, and one-path normal commits under `commit_lock`.
- [x] For write failure, remove newly created immutable paths and restore the affected Git index path before releasing `commit_lock`.
- [x] Re-run store tests and `cargo test -p gitim-daemon --lib --locked`.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 4: Daemon IPC handlers

**Files:**

- Create: `crates/gitim-daemon/src/skill_handlers.rs`
- Modify: `crates/gitim-daemon/src/api.rs`
- Modify: `crates/gitim-daemon/src/handlers/mod.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`
- Test: `crates/gitim-daemon/tests/skill_handlers.rs`

**Interfaces:**

- Consumes `SkillStore` methods.
- Produces IPC methods `skill_list`, `skill_show`, `skill_load`, `skill_resource`, `skill_revisions`, `skill_history`, `skill_validate`, `skill_create`, `skill_propose`, proposal operations, metadata operations, role operations, and archive operations.

- [x] Write failing handler tests that dispatch serialized requests and assert stable response bodies/error codes for reads, permissions, stale/competing proposal decisions, invalid history isolation, and idempotent event IDs.
- [x] Run `cargo test -p gitim-daemon --test skill_handlers --locked` and verify missing request variants.
- [x] Add typed request variants, write classification, author resolution, response serialization, and one bounded SSE `skill_changed` event after successful mutations.
- [x] Re-run handler tests plus `cargo test -p gitim-daemon --lib --locked`.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 5: Client and CLI

**Files:**

- Modify: `crates/gitim-client/src/client.rs`
- Create: `crates/gitim-cli/src/commands/skill.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Modify: `crates/gitim-cli/src/main.rs`
- Test: `crates/gitim-cli/tests/skill_cli.rs`

**Interfaces:**

- Consumes daemon IPC methods.
- Produces the command tree in `00-requirements.md`, human output, global JSON output, binary-safe resource export, and optional `--event-id` on writes.

- [x] Write failing clap/parser tests for progressive nested help and every command family.
- [x] Write subprocess CLI tests against a temporary socket for request shapes, progressive help, JSON cleanliness, binary output refusal, and output overwrite protection; cover lifecycle, permissions, pagination, and archive behavior in daemon tests.
- [x] Run `cargo test -p gitim-cli --test skill_cli --locked` and verify expected failures.
- [x] Implement thin client methods and a focused CLI module. Resolve `--from` to an absolute directory before sending it to the daemon; never inline package bytes into the base prompt or command help.
- [x] Re-run `skill_cli`, existing `quick_session_cli`, and `timer_cli` targets.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 6: Minimal provider discovery contract

**Files:**

- Modify: `crates/gitim-agent-provider/src/prompts.rs`
- Test: `crates/gitim-agent-provider/tests/prompt_test.rs`

**Interfaces:**

- Produces one stable four-line Skill contract through the existing shared GitIM API prompt.

- [x] Add a failing prompt test asserting the four required concepts and rejecting catalog contents, package bodies, and management command enumeration.
- [x] Run `cargo test -p gitim-agent-provider --test prompt_test --locked` and verify failure.
- [x] Add the minimal prompt text.
- [x] Re-run provider prompt tests.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.

### Task 7: Cross-layer verification and review

**Files:**

- Modify only files required by findings from verification and review.

- [x] Run `cargo test -p gitim-core --locked`.
- [x] Run `cargo test -p gitim-daemon --lib --test skill_store --test skill_handlers --locked`.
- [x] Run `cargo test -p gitim-cli --test skill_cli --test quick_session_cli --test timer_cli --locked`.
- [x] Run `cargo test -p gitim-agent-provider --test prompt_test --locked`.
- [x] Run `cargo clippy --workspace --all-targets --no-deps --locked`.
- [x] Run `cargo fmt --all -- --check` and `git diff --check`.
- [x] Because this adds shared protocol/API variants, run final `cargo test --workspace --locked` once after scoped tests pass.
- [x] Review requirements coverage, production/test line counts, and verify the implementation stays within the core, daemon, client, CLI, and provider boundaries listed above.
- [x] Perform a specification review, then a code-quality review; fix all high-confidence findings and repeat affected tests.
- [x] Commit with a Conventional Commit title, `Test:` footer, and `Co-authored-by: Codex <codex@openai.com>` trailer.
