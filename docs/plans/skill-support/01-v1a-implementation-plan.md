# GitIM Shared Skills v1a Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ship the first mergeable shared-Skill slice: immutable Agent
Skills-compatible packages, validated Git history, remote fast-forward
transactions, daemon/client/CLI creation and review, and explicit pinned loading
by agents.

**Architecture:** `gitim-core` owns schemas, package/ref parsing, mutation
planning, and transition validation. `gitim-sync` owns private-index Git object
construction, guarded working-branch publication/integration, checkpoints,
quarantine replay, epoch gates, and remote CAS retries. The daemon imports local
directories, resolves actors, maintains accepted read views, and exposes typed
requests; client/CLI and the provider prompt remain thin consumers.

**Tech Stack:** Stable Rust, serde/serde_yaml, sha2, ulid, Git plumbing
subprocesses, Tokio `spawn_blocking`, fs2/tempfile atomic local state, clap,
wasm-bindgen, existing newline-delimited daemon IPC.

## Global Constraints

- Work only in `/Users/lewisliu/ateam/GitIM/.codex/skill-support` on
  `codex/skill-support`.
- Do not write or link runtime-native Skill directories.
- Provider prompts contain only the stable load/help contract; never inject a
  catalog, command tree, or Skill body.
- Store the exact validated `SKILL.md` bytes and preserve unknown frontmatter.
- `SKILL.md` is at most 64 KiB; each file at most 5 MiB; at most 256 files; at
  most 10 MiB total.
- Loading is explicit and never executes scripts or changes provider
  permissions.
- Remote-backed mutations require the remote and publish only through a normal
  fast-forward ref update.
- Request, revision, and proposal IDs are generated once and retained across
  retries.
- Every mutation creates one root `skills/receipts/q-<ulid>.meta.yaml` receipt
  and one attributed Git commit; reads create neither.
- The stable Rust toolchain is mandatory; UTF-8 byte truncation uses
  `is_char_boundary`.
- Every task uses TDD, runs scoped tests, receives Cursor Grok plus Kimi k3
  review, folds accepted corrections, and commits with `Test:` and
  `Co-authored-by: Codex <codex@openai.com>`.

---

## File Structure

### Core domain

- Create `crates/gitim-core/src/skill/mod.rs` — public Skill domain facade.
- Create `crates/gitim-core/src/skill/error.rs` — stable `SkillError` codes and
  structured stale/conflict details.
- Create `crates/gitim-core/src/skill/id.rs` — slug and ULID-prefixed identifier
  newtypes.
- Create `crates/gitim-core/src/skill/types.rs` — repository metadata, receipts,
  proposals, publications, DTOs, and operation/result enums.
- Create `crates/gitim-core/src/skill/reference.rs` — canonical ref parser and
  message scanner.
- Create `crates/gitim-core/src/skill/package.rs` — path/frontmatter validation,
  canonical manifest hashing, media types, and stable truncation.
- Create `crates/gitim-core/src/skill/transition.rs` — pure mutation planner and
  before/after commit validator.
- Create `crates/gitim-core/tests/skill_protocol.rs` — schema, ID, ref, and
  serialization contracts.
- Create `crates/gitim-core/tests/skill_package.rs` — exact-byte/path/hash/bounds
  tests.
- Create `crates/gitim-core/tests/skill_transition.rs` — permission, revision,
  receipt, proposal, and allowed-path transitions.

### Git safety and transactions

- Create `crates/gitim-sync/src/skill/mod.rs` — native Skill Git facade.
- Create `crates/gitim-sync/src/skill/git_tree.rs` — explicit-ref tree/blob
  reads and private-index commit building.
- Create `crates/gitim-sync/src/skill/checkpoint.rs` — atomic accepted checkpoint
  and conflict state.
- Create `crates/gitim-sync/src/skill/guard.rs` — `guarded_push`,
  `guarded_integrate`, bypass quarantine/replay, and cross-object archive checks.
- Create `crates/gitim-sync/src/skill/transaction.rs` — request journal,
  semantic read set, CAS retry, recovery, and accepted-read result.
- Create `crates/gitim-sync/tests/skill_git_tree.rs` — object plumbing isolation.
- Create `crates/gitim-sync/tests/skill_guard.rs` — every push/integration seam,
  bypass replay, and invalid incoming history.
- Create `crates/gitim-sync/tests/skill_transaction.rs` — two-writer races,
  crash phases, retry IDs, epoch switches, and repair.
- Modify `crates/gitim-sync/src/git.rs` — reusable env-aware plumbing and
  arbitrary-commit push; restrict raw working-branch push.
- Modify `crates/gitim-sync/src/sync_loop.rs` — route all publication,
  integration, replay, divergence, and epoch paths through the Skill guard.
- Modify `crates/gitim-sync/src/rotate.rs` — block rotation on unhealthy Skill
  state and validate epoch rollover.
- Modify `crates/gitim-daemon/src/handlers/{channel.rs,dm.rs,user.rs,depart.rs}`
  — replace direct working-branch pushes with `guarded_push`.
- Modify `crates/gitim-daemon/src/{card_handlers.rs,onboard.rs,reconcile.rs}` —
  replace direct working-branch pushes and the shared card/project/label retry
  helper with `guarded_push`.
- Modify `crates/gitim-daemon/src/handlers/{project.rs,labels.rs}` — consume the
  guarded shared retry helper; no raw `GitStorage::push` remains callable
  outside the Skill guard and explicit private-ref/epoch plumbing.

### Daemon, Runtime, client, and CLI

- Create `crates/gitim-daemon/src/skill_import.rs` — no-symlink local directory
  snapshot.
- Create `crates/gitim-daemon/src/skill_store.rs` — worktree/accepted-commit
  catalog and bounded blob reads.
- Create `crates/gitim-daemon/src/skill_handlers.rs` — read and v1a mutation
  handlers.
- Create `crates/gitim-daemon/tests/skill_handlers.rs` — local and remote handler
  integration.
- Create `crates/gitim-daemon/tests/skill_sync_safety.rs` — direct handler pushes,
  remote invalid commits, SSE, and departure race.
- Modify `crates/gitim-daemon/src/api.rs` — Skill request/event variants.
- Modify `crates/gitim-daemon/src/state.rs` — guard, accepted view, semaphore,
  rotation gate, and synced-change callback.
- Modify `crates/gitim-daemon/src/handlers/mod.rs` — dispatch and actor
  resolution.
- Modify direct-push handlers under `crates/gitim-daemon/src/handlers/`,
  `card_handlers.rs`, `onboard.rs`, and `reconcile.rs` — use `guarded_push`.
- Modify `crates/gitim-daemon/src/main.rs` — incoming Skill invalidation events.
- Modify `crates/gitim-runtime/src/http.rs` — perform tracked administrator
  bootstrap on new-workspace creation and recovery, expose health counters,
  and add only the loopback administrator repair endpoint; public Skill HTTP
  waits for v1b.
- Create `crates/gitim-runtime/src/cli/cmd_skill_repair.rs` and modify
  `crates/gitim-runtime/src/cli/{mod.rs,dto.rs,http.rs}` plus
  `crates/gitim-runtime/src/bin/runtime.rs` — expose
  `gitim-runtime repair-skill-state` without adding it to the agent-facing CLI.
- Create `crates/gitim-runtime/tests/skill_admin_repair.rs` — checkpoint-bound
  workspace/Skill repair and Runtime CLI behavior.
- Modify `crates/gitim-client/src/client.rs` — typed Skill convenience calls.
- Create `crates/gitim-cli/src/commands/skill.rs` — progressive read/write
  commands and bounded output.
- Modify `crates/gitim-cli/src/commands/mod.rs` and
  `crates/gitim-cli/src/main.rs` — nested `skill` command tree.
- Create `crates/gitim-cli/tests/skill_cli.rs` — argv/help/output/exit behavior.

### Shared browser logic and agent loading

- Modify `crates/gitim-wasm/src/lib.rs` — Skill ref/schema/media exports.
- Modify `crates/gitim-agent-provider/src/prompts.rs` — four-line stable Skill
  entry point.
- Modify `crates/gitim-agent-provider/tests/prompt_test.rs` — prompt bounds and
  omissions.
- Create `crates/gitim-runtime/tests/skill_agent_load.rs` — mock-provider pinned
  ref load end-to-end.

---

### Task 1: Skill identifiers, schemas, errors, and references

**Files:**

- Create: `crates/gitim-core/src/skill/{mod.rs,error.rs,id.rs,types.rs,reference.rs}`
- Modify: `crates/gitim-core/src/lib.rs`
- Modify: `crates/gitim-core/Cargo.toml`
- Modify: `Cargo.toml`
- Test: `crates/gitim-core/tests/skill_protocol.rs`

**Interfaces:**

- Produces: `SkillSlug`, `RevisionId`, `ProposalId`, `RequestId`,
  `SkillReference`, `SkillError`, metadata structs, `SkillOperation`,
  `SkillMutationResult`, `parse_skill_reference`, and
  `scan_skill_references`.
- Consumes: existing `Handler`, serde, chrono, getrandom, and a new workspace
  `ulid = { version = "1", default-features = false }` dependency. Generate
  through `Ulid::from_parts` plus the existing wasm-compatible
  `preconditions::random_bytes`; the default `ulid/std` feature pulls rand
  0.9/getrandom 0.3 and is forbidden because it breaks
  `wasm32-unknown-unknown`.

- [ ] **Step 1: Write failing protocol tests**

```rust
use gitim_core::skill::{
    parse_skill_reference, scan_skill_references, ProposalId, RequestId, RevisionId,
    SkillError, SkillReference, SkillSlug,
};

#[test]
fn identifiers_are_prefixed_uppercase_ulids() {
    let revision = RevisionId::generate();
    let proposal = ProposalId::generate();
    let request = RequestId::generate();
    assert!(revision.as_str().starts_with("r-"));
    assert!(proposal.as_str().starts_with("p-"));
    assert!(request.as_str().starts_with("q-"));
    assert_eq!(revision.as_str().len(), 28);
}

#[test]
fn canonical_reference_round_trips() {
    let parsed = parse_skill_reference(
        "skill:release-check@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N",
    ).unwrap();
    assert_eq!(parsed.slug.as_str(), "release-check");
    assert_eq!(
        parsed.revision.unwrap().as_str(),
        "r-01K1D8QG2S8RX4T9M9BDKQ9Z7N"
    );
}

#[test]
fn scanner_ignores_code_urls_and_escapes() {
    let refs = scan_skill_references(
        r#"`skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N`
https://x/skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N
\skill:nope@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N
skill:ok@r-01K1D8QG2S8RX4T9M9BDKQ9Z7N"#,
    );
    assert_eq!(refs.len(), 1);
    assert_eq!(refs[0].slug.as_str(), "ok");
}

#[test]
fn errors_have_stable_codes() {
    assert_eq!(SkillError::NotFound.code(), "skill_not_found");
    assert_eq!(SkillError::RequestIdConflict.code(), "request_id_conflict");
}
```

- [ ] **Step 2: Run the tests and verify the missing module failure**

Run:

```bash
cargo test -p gitim-core --test skill_protocol --locked
```

Expected: compile failure because `gitim_core::skill` does not exist.

- [ ] **Step 3: Implement exact public types**

Use these signatures:

```rust
pub const SKILL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SkillSlug(String);

impl SkillSlug {
    pub fn new(value: &str) -> Result<Self, SkillError>;
    pub fn as_str(&self) -> &str;
}

macro_rules! prefixed_ulid_id {
    ($name:ident, $prefix:literal) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);
        impl $name {
            pub fn new(value: &str) -> Result<Self, SkillError>;
            pub fn generate() -> Self;
            pub fn as_str(&self) -> &str;
        }
    };
}

prefixed_ulid_id!(RevisionId, "r-");
prefixed_ulid_id!(ProposalId, "p-");
prefixed_ulid_id!(RequestId, "q-");

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SkillReference {
    pub slug: SkillSlug,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<RevisionId>,
}
```

`types.rs` must define the YAML field names from `00-requirements.md` without
renaming or omission: `WorkspaceSkillMeta`, `SkillMeta`,
`SkillRevisionMeta`, `SkillPublicationMeta`, `SkillProposalMeta`,
`SkillReceipt`, `SkillReceiptScope`, `ProposalStatus`, `SkillOperation`,
`SkillMutationRequest`, `SkillMutationResult`, and bounded catalog/load DTOs.
`SkillError` is the only stable error-code source and provides
`code() -> &'static str`.

The bounded DTO names used by later crates are fixed here:

```rust
pub struct SkillListQuery {
    pub archived: bool,
    pub limit: u16,
    pub cursor: Option<String>,
}

pub struct SkillListResponse {
    pub skills: Vec<SkillCatalogEntry>,
    pub next_cursor: Option<String>,
}

pub struct ResourceDescriptor {
    pub path: String,
    pub byte_size: u64,
    pub media_type: String,
    pub text: bool,
}

pub struct SkillShowQuery {
    pub slug: SkillSlug,
    pub revision: Option<RevisionId>,
}

pub struct SkillShowResponse {
    pub meta: SkillMeta,
    pub revision: SkillRevisionMeta,
    pub canonical_ref: SkillReference,
    pub archived: bool,
}

pub struct SkillLoadResponse {
    pub canonical_ref: SkillReference,
    pub revision: SkillRevisionMeta,
    pub skill_markdown: String,
    pub resources: Vec<ResourceDescriptor>,
    pub archived: bool,
}

pub struct SkillResourceQuery {
    pub reference: SkillReference,
    pub path: String,
}

pub struct SkillResourceResponse {
    pub canonical_ref: SkillReference,
    pub path: String,
    pub media_type: String,
    pub text: bool,
    pub bytes: Vec<u8>,
}

pub struct SkillCreateRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub display_name: String,
    pub description: String,
    pub source_directory: PathBuf,
}

pub struct SkillProposeRequest {
    pub request_id: RequestId,
    pub slug: SkillSlug,
    pub base_revision: RevisionId,
    pub summary: String,
    pub source_directory: PathBuf,
}

pub struct SkillProposalTransitionRequest {
    pub request_id: RequestId,
    pub proposal_id: ProposalId,
    pub operation: SkillOperation,
    pub expected_state_revision: u64,
    pub expected_control_revision: Option<u64>,
}
```

`SkillCatalogEntry` contains only slug, display name, description, current
revision, owners, maintainers, open proposal count, and archived state. Core
DTOs carry package source paths only on the local request boundary; receipts
and repository metadata store content fingerprints, never local paths.

`SkillError::code()` is exhaustive over this exact wire set:
`skill_not_found`, `skill_archived`, `skill_exists`, `skill_invalid_slug`,
`skill_invalid_package`, `skill_package_too_large`,
`skill_revision_not_found`, `skill_revision_unpublished`,
`skill_revision_corrupted`, `skill_proposal_not_found`,
`skill_proposal_terminal`, `skill_open_proposal_limit`,
`skill_stale_content_revision`, `skill_stale_control_revision`,
`skill_stale_proposal_revision`, `skill_not_maintainer`, `skill_not_owner`,
`skill_admin_required`, `skill_admin_uninitialized`, `skill_last_admin`,
`skill_admin_role_present`, `skill_last_owner`,
`skill_owner_is_maintainer`, `skill_role_target_invalid`,
`skill_role_target_inactive`, `skill_roles_present`, `skill_remote_required`,
`skill_sync_conflict`, `skill_local_quarantine_blocked`,
`skill_epoch_validation_blocked`, `skill_load_unavailable`,
`request_id_conflict`, `output_exists`, and `stale_cursor`. Stale variants carry
the current content/control/event/proposal values needed by the relevant
operation instead of embedding those values in error strings.

The scanner is a single-pass state machine with fenced-code, inline-code,
escaped-character, and Markdown-link-destination states; it does not use one
permissive global regex.

- [ ] **Step 4: Run scoped tests**

Run:

```bash
cargo check -p gitim-core
cargo test -p gitim-core --test skill_protocol --locked
cargo test -p gitim-core --locked
task_rustc_path="$(rustup which rustc --toolchain stable)"
RUSTC="$task_rustc_path" cargo check -p gitim-core \
  --target wasm32-unknown-unknown --locked
```

Expected: the first command records the new `ulid` dependency in `Cargo.lock`;
all locked tests and the wasm target check then pass.

- [ ] **Step 5: Run external review and fold corrections**

Run both from the worktree:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print \
  --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 1 Skill protocol diff. Do not modify files. Report only correctness, compatibility, or test gaps."

/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 1 Skill protocol diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Apply accepted fixes and rerun Step 4. Cursor connection failure is recorded and
does not block Kimi plus deterministic verification.

- [ ] **Step 6: Commit**

```bash
git add docs/plans/skill-support Cargo.toml Cargo.lock crates/gitim-core
git commit -m "feat(core): add shared skill protocol types" \
  -m "Test: cargo test -p gitim-core --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 2: Exact-byte package validation and bounded output

**Files:**

- Create: `crates/gitim-core/src/skill/package.rs`
- Modify: `crates/gitim-core/src/skill/mod.rs`
- Modify: `crates/gitim-core/Cargo.toml`
- Test: `crates/gitim-core/tests/skill_package.rs`

**Interfaces:**

- Consumes: `SkillSlug`, `SkillError`.
- Consumes: `ResourceDescriptor`.
- Produces: `PackageEntry`, `ValidatedPackage`,
  `validate_package_entries`, `canonical_package_sha256`,
  `media_type_for_path`, and `truncate_utf8_bytes`.

- [ ] **Step 1: Write failing exact-byte and boundary tests**

```rust
use gitim_core::skill::{
    canonical_package_sha256, truncate_utf8_bytes, validate_package_entries,
    PackageEntry, SkillSlug,
};

#[test]
fn preserves_skill_markdown_bytes_and_unknown_frontmatter() {
    let raw = b"---\nname: release-check\ndescription: Check release\nx-runtime: keep\n---\nBody\n";
    let package = validate_package_entries(
        &SkillSlug::new("release-check").unwrap(),
        vec![PackageEntry::new("SKILL.md", raw.to_vec())],
    ).unwrap();
    assert_eq!(package.skill_markdown, raw);
}

#[test]
fn manifest_hash_is_order_independent() {
    let a = PackageEntry::new("SKILL.md", b"---\nname: x\ndescription: y\n---\n".to_vec());
    let b = PackageEntry::new("references/a.md", b"a".to_vec());
    assert_eq!(
        canonical_package_sha256(&[a.clone(), b.clone()]).unwrap(),
        canonical_package_sha256(&[b, a]).unwrap()
    );
}

#[test]
fn truncation_keeps_utf8_boundary() {
    assert_eq!(truncate_utf8_bytes("a🙂b", 3), "a");
    assert_eq!(truncate_utf8_bytes("a🙂b", 5), "a🙂");
}
```

Add table-driven rejection tests for symlinks represented as non-regular entry
kinds, traversal, absolute paths, backslashes, case-fold collisions, reserved
Windows segments, NUL/control characters, 81-character segments, 241-byte
paths, missing/invalid `SKILL.md`, frontmatter name mismatch, 257 files, 5 MiB
plus one byte, and 10 MiB plus one byte.

- [ ] **Step 2: Confirm failure**

Run:

```bash
cargo test -p gitim-core --test skill_package --locked
```

Expected: compile failure for missing package APIs.

- [ ] **Step 3: Implement the bounded package API**

```rust
pub const MAX_SKILL_MD_BYTES: usize = 64 * 1024;
pub const MAX_PACKAGE_FILE_BYTES: usize = 5 * 1024 * 1024;
pub const MAX_PACKAGE_FILES: usize = 256;
pub const MAX_PACKAGE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageEntry {
    pub path: String,
    pub bytes: Vec<u8>,
}

impl PackageEntry {
    pub fn new(path: impl Into<String>, bytes: Vec<u8>) -> Self;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPackage {
    pub entries: Vec<PackageEntry>,
    pub skill_markdown: Vec<u8>,
    pub content_sha256: String,
    pub resources: Vec<ResourceDescriptor>,
}

pub fn validate_package_entries(
    slug: &SkillSlug,
    entries: Vec<PackageEntry>,
) -> Result<ValidatedPackage, SkillError>;

pub fn truncate_utf8_bytes(value: &str, max_bytes: usize) -> &str {
    let mut end = value.len().min(max_bytes);
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    &value[..end]
}
```

Hash the exact canonical stream
`u32_be(path_len) + path + u64_be(file_len) + bytes`, sorted by validated path.
Parse only the first YAML frontmatter block for `name` and `description`; retain
the original byte vector. Add `sha2.workspace = true`.

- [ ] **Step 4: Verify**

Run:

```bash
cargo test -p gitim-core --test skill_package --locked
cargo test -p gitim-core --locked
```

Expected: all tests pass and no nightly feature is used.

- [ ] **Step 5: External review, fixes, and commit**

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 2 package-validation diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 2 package-validation diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun Step 4, then:

```bash
git add Cargo.lock crates/gitim-core
git commit -m "feat(core): validate shared skill packages" \
  -m "Test: cargo test -p gitim-core --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 3: Pure mutation planning and transition validation

**Files:**

- Create: `crates/gitim-core/src/skill/transition.rs`
- Modify: `crates/gitim-core/src/skill/{mod.rs,types.rs,error.rs}`
- Test: `crates/gitim-core/tests/skill_transition.rs`

**Interfaces:**

- Consumes: typed metadata, validated package, actor/active-user facts, before
  repository snapshot, and typed request.
- Produces: `plan_skill_mutation` and `validate_skill_commit`.

- [ ] **Step 1: Write failing transition matrix tests**

Create fixtures for an empty workspace, initialized workspace, active Skill with
one open proposal, and archived Skill. Assert:

```rust
let plan = plan_skill_mutation(&before, &context, &request).unwrap();
assert_eq!(plan.receipt.id, request.request_id());
assert_eq!(plan.changed_paths, expected_paths);
validate_skill_commit(&before, &plan.after, &plan.commit_evidence).unwrap();
```

Cover `workspace_bootstrap`, `skill_create`, `proposal_create`,
`proposal_publish`, `proposal_reject`, and `proposal_withdraw`; duplicate
matching request returns the recorded result, mismatched reuse returns
`RequestIdConflict`; stale content/control/state revisions return structured
current values; create publishes its initial revision; candidate-only revisions
never enter publications; terminal proposal transitions decrement
`open_proposal_count` and remove the ID from `open_proposal_ids`. Add
`repair_skill_state` cases for workspace and Skill scope: only a tracked
administrator may restore the exact accepted tree named by the local conflict
checkpoint; the repair receipt records conflict tip and accepted tree and may
not copy bytes from the rejected tree.

- [ ] **Step 2: Confirm failure**

Run:

```bash
cargo test -p gitim-core --test skill_transition --locked
```

Expected: compile failure for missing transition APIs.

- [ ] **Step 3: Implement one mutation/validation authority**

```rust
pub struct SkillRepositorySnapshot {
    pub workspace: Option<WorkspaceSkillMeta>,
    pub active_skills: BTreeMap<SkillSlug, SkillObjectSnapshot>,
    pub archived_skills: BTreeMap<SkillSlug, SkillObjectSnapshot>,
    pub receipts: BTreeMap<RequestId, SkillReceipt>,
    pub active_users: BTreeSet<String>,
}

pub struct SkillObjectSnapshot {
    pub meta: SkillMeta,
    pub revisions: BTreeMap<RevisionId, SkillRevisionSnapshot>,
    pub publications: BTreeMap<RevisionId, SkillPublicationMeta>,
    pub proposals: BTreeMap<ProposalId, SkillProposalSnapshot>,
    pub history: String,
}

pub struct SkillRevisionSnapshot {
    pub meta: SkillRevisionMeta,
    pub package: ValidatedPackage,
}

pub struct SkillProposalSnapshot {
    pub meta: SkillProposalMeta,
    pub discussion: String,
}

pub enum SkillTreeEdit {
    Upsert { path: String, bytes: Vec<u8> },
    Delete { path: String },
}

pub struct SkillMutationContext {
    pub actor: String,
    pub now: String,
    pub package: Option<ValidatedPackage>,
}

pub struct SkillMutationPlan {
    pub after: SkillRepositorySnapshot,
    pub edits: Vec<SkillTreeEdit>,
    pub receipt: SkillReceipt,
    pub result: SkillMutationResult,
    pub changed_paths: BTreeSet<String>,
    pub commit_message: String,
}

pub struct SkillCommitEvidence {
    pub commit_author: String,
    pub request_trailer: RequestId,
    pub parent_count: usize,
    pub receipt: SkillReceipt,
    pub changed_paths: BTreeSet<String>,
}

pub struct SkillTransitionOutcome {
    pub changed_skill: Option<SkillSlug>,
    pub event_revision: Option<u64>,
    pub control_revision: Option<u64>,
}

pub fn plan_skill_mutation(
    before: &SkillRepositorySnapshot,
    context: &SkillMutationContext,
    request: &SkillMutationRequest,
) -> Result<SkillMutationPlan, SkillError>;

pub fn validate_skill_commit(
    before: &SkillRepositorySnapshot,
    after: &SkillRepositorySnapshot,
    evidence: &SkillCommitEvidence,
) -> Result<SkillTransitionOutcome, SkillError>;
```

`plan_skill_mutation` must call the same invariant functions that
`validate_skill_commit` calls; it may not maintain a second permission table.
The evidence contains commit author, `Gitim-Request-Id`, exact changed paths,
receipt, and merge/single-parent status.

- [ ] **Step 4: Verify and review**

Run:

```bash
cargo test -p gitim-core --test skill_transition --locked
cargo test -p gitim-core --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 3 transition diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 3 transition diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections and rerun the verification commands.

- [ ] **Step 5: Commit**

```bash
git add crates/gitim-core
git commit -m "feat(core): validate shared skill transitions" \
  -m "Test: cargo test -p gitim-core --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 4: WASM parity for refs, metadata, and media types

**Files:**

- Modify: `crates/gitim-wasm/src/lib.rs`
- Modify/generated: `crates/gitim-wasm/pkg/`
- Test: Rust wasm-facing unit tests in `crates/gitim-wasm/src/lib.rs`

**Interfaces:**

- Consumes: core ref/schema/media functions.
- Produces JS exports `parseSkillReference`, `scanSkillReferences`,
  `parseSkillMeta`, and `skillMediaType`.

- [ ] **Step 1: Add failing export tests**

Assert Rust wrappers return serialized core values and preserve stable error
strings for malformed refs and metadata.

- [ ] **Step 2: Implement thin wrappers**

```rust
#[wasm_bindgen(js_name = "parseSkillReference")]
pub fn parse_skill_reference_wasm(value: &str) -> Result<JsValue, JsError>;

#[wasm_bindgen(js_name = "scanSkillReferences")]
pub fn scan_skill_references_wasm(value: &str) -> Result<JsValue, JsError>;

#[wasm_bindgen(js_name = "parseSkillMeta")]
pub fn parse_skill_meta_wasm(yaml: &str) -> Result<JsValue, JsError>;

#[wasm_bindgen(js_name = "skillMediaType")]
pub fn skill_media_type_wasm(path: &str) -> String;
```

No validation rule is reimplemented in TypeScript.

- [ ] **Step 3: Verify Rust and generated wasm**

Run:

```bash
cargo test -p gitim-wasm --locked
cd products/gitim/frontend
npm run build:wasm
cd /Users/lewisliu/ateam/GitIM/.codex/skill-support
git diff --check
```

Expected: tests/build pass and generated output changes only through the wasm
build.

- [ ] **Step 4: External review and commit**

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 4 wasm parity diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 4 wasm parity diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun Step 3, then:

```bash
git add crates/gitim-wasm products/gitim/frontend
git commit -m "feat(wasm): expose shared skill protocol" \
  -m "Test: cargo test -p gitim-wasm --locked; npm run build:wasm" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 5: Explicit-ref Git tree reads and private-index commit building

**Files:**

- Create: `crates/gitim-sync/src/skill/{mod.rs,git_tree.rs}`
- Modify: `crates/gitim-sync/src/{lib.rs,git.rs}`
- Modify: `crates/gitim-sync/Cargo.toml`
- Test: `crates/gitim-sync/tests/skill_git_tree.rs`

**Interfaces:**

- Consumes: `SkillTreeEdit`, explicit base commit, actor identity.
- Produces: tree/blob reads, semantic path OIDs, and candidate commit OID without
  changing active branch/index/worktree.

- [ ] **Step 1: Write failing real-repository tests**

Use local integration-test helpers following
`crates/gitim-sync/tests/rotate_test.rs` and
`crates/gitim-sync/tests/empty_remote_push.rs` to create a repository, seed a
base commit, dirty the worktree and index, then call:

```rust
let built = build_private_index_commit(
    &repo,
    &PrivateIndexCommitRequest {
        base_commit: base.clone(),
        private_index: temp.path().join("index"),
        edits,
        message: "skill: create release-check".into(),
        author_name: "alice".into(),
        author_email: "alice@example.com".into(),
        request_id: request_id.clone(),
    },
).unwrap();
```

Assert candidate parent equals base; candidate tree contains edits; `HEAD`,
`.git/index`, staged diff, worktree bytes, and upstream config are unchanged.
Assert `push_commit_fast_forward` pushes an arbitrary commit without `-u` and a
stale base returns the existing push-conflict classification.

- [ ] **Step 2: Confirm failure**

Run:

```bash
cargo test -p gitim-sync --test skill_git_tree --locked
```

Expected: compile failure for missing module/functions.

- [ ] **Step 3: Implement plumbing**

```rust
pub struct GitTreeEntry {
    pub mode: String,
    pub object_type: String,
    pub oid: String,
    pub path: String,
}

pub struct PrivateIndexCommitRequest {
    pub base_commit: String,
    pub private_index: PathBuf,
    pub edits: Vec<SkillTreeEdit>,
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    pub request_id: RequestId,
}

pub struct BuiltPrivateCommit {
    pub commit_oid: String,
    pub tree_oid: String,
}

pub fn tree_oid_at(repo: &GitStorage, commit: &str, path: &str)
    -> Result<Option<String>, GitError>;
pub fn read_blob_at(repo: &GitStorage, commit: &str, path: &str)
    -> Result<Option<Vec<u8>>, GitError>;
pub fn list_tree_recursive(repo: &GitStorage, commit: &str, path: &str)
    -> Result<Vec<GitTreeEntry>, GitError>;
pub fn build_private_index_commit(
    repo: &GitStorage,
    request: &PrivateIndexCommitRequest,
) -> Result<BuiltPrivateCommit, GitError>;
pub fn push_commit_fast_forward(
    repo: &GitStorage,
    commit: &str,
    remote_branch: &str,
) -> Result<(), GitError>;
```

Every `read-tree`, `update-index`, and `write-tree` invocation receives the
private `GIT_INDEX_FILE`. `commit-tree` receives author/committer environment and
`Gitim-Request-Id: <id>` in the message. Add native `tempfile` and required
serialization dependencies under
`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`, and gate
`gitim_sync::skill` from `lib.rs` with the same `cfg`. `gitim-wasm` depends on
`gitim-sync`, so no filesystem/process dependency may enter the wasm target.

- [ ] **Step 4: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-sync --test skill_git_tree --locked
cargo test -p gitim-sync --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 5 private-index Git diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 5 private-index Git diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add Cargo.lock crates/gitim-sync
git commit -m "feat(sync): build skill commits with private indexes" \
  -m "Test: cargo test -p gitim-sync --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 6: Accepted checkpoints and incoming transition validation

**Files:**

- Create: `crates/gitim-sync/src/skill/checkpoint.rs`
- Modify: `crates/gitim-sync/src/skill/mod.rs`
- Modify: `crates/gitim-sync/Cargo.toml`
- Test: `crates/gitim-sync/tests/skill_guard.rs`

**Interfaces:**

- Consumes: fetched tip, retained epoch lineage, core transition validator.
- Produces: atomic `SkillValidationCheckpoint`, per-Skill accepted tree/read
  view, conflict state, and accepted change list.

- [ ] **Step 1: Write failing checkpoint/history tests**

Build Git histories for valid single-parent mutations, missing/mismatched root
receipts, illegal path sets, merge commits touching Skill paths, corrupted
revision hashes, non-fast-forward remote rewrites, lagging epoch followers, and
an authorized repair commit. Assert invalid intermediate trees are never
returned as accepted.

- [ ] **Step 2: Implement checkpoint types and atomic persistence**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillValidationCheckpoint {
    pub schema_version: u32,
    pub active_epoch: String,
    pub last_scanned_tip: String,
    pub workspace_tree: Option<AcceptedTree>,
    pub skills: BTreeMap<String, AcceptedSkillState>,
    pub conflicts: BTreeMap<String, SkillConflict>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptedTree {
    pub commit_oid: String,
    pub tree_oid: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AcceptedSkillState {
    pub tree: AcceptedTree,
    pub event_revision: u64,
    pub archived: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SkillConflict {
    pub rejected_commit: String,
    pub code: String,
    pub accepted_tree_oid: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AcceptedSkillChange {
    pub slug: String,
    pub event_revision: u64,
    pub control_revision: u64,
}

#[derive(Clone)]
pub struct SkillCheckpointStore {
    pub path: PathBuf,
    pub lock_path: PathBuf,
}

#[derive(Debug, Error)]
pub enum SkillSyncError {
    Git(#[from] GitError),
    Domain(#[from] SkillError),
    Checkpoint(String),
    LocalQuarantineBlocked(String),
    EpochValidationBlocked(String),
}

pub struct IncomingSkillValidation {
    pub checkpoint: SkillValidationCheckpoint,
    pub accepted_changes: Vec<AcceptedSkillChange>,
}

pub fn validate_incoming_skill_history(
    repo: &GitStorage,
    previous: &SkillValidationCheckpoint,
    fetched_tip: &str,
) -> Result<IncomingSkillValidation, SkillSyncError>;
```

Persist to `.gitim/skill-validation.json` with a separate
`.gitim/skill-validation.json.lock`, fs2 exclusive lock, same-directory
temporary file, fsync, and atomic persist. Fresh clones walk retained epoch
predecessors to the initial no-Skill state; lagging followers validate the
sealed predecessor through its seal before comparing the orphan root. Add
`fs2` and `tempfile` only to gitim-sync's non-wasm target dependencies; the
entire checkpoint module remains under the native `gitim_sync::skill` gate.

- [ ] **Step 3: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-sync --test skill_guard --locked
cargo test -p gitim-sync --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 6 checkpoint validator diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 6 checkpoint validator diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then commit:

```bash
git add Cargo.lock crates/gitim-sync
git commit -m "feat(sync): validate incoming skill history" \
  -m "Test: cargo test -p gitim-sync --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 7: Central guarded push/integration and bypass replay

**Files:**

- Create: `crates/gitim-sync/src/skill/guard.rs`
- Modify: `crates/gitim-sync/src/{git.rs,sync_loop.rs,rotate.rs}`
- Modify: `crates/gitim-daemon/src/handlers/{channel.rs,dm.rs,user.rs,depart.rs,project.rs,labels.rs}`
- Modify: `crates/gitim-daemon/src/{card_handlers.rs,onboard.rs,reconcile.rs}`
- Modify: `crates/gitim-daemon/src/state.rs`
- Test: `crates/gitim-sync/tests/skill_guard.rs`
- Test: `crates/gitim-daemon/tests/skill_sync_safety.rs`

**Interfaces:**

- Consumes: checkpoint manager, existing content-aware replay, commit lock,
  author identity.
- Produces: the only working-branch `guarded_push` and `guarded_integrate`
  surfaces.

- [ ] **Step 1: Add failing choke-point and liveness tests**

Tests must enumerate all current direct `.push()` call sites by behavior:
user/channel/DM archive, onboard, card, reconcile, sync initial/rebase/resolve,
and epoch replay. Seed a bypassed Skill commit followed by message and card
commits; assert a durable `refs/gitim/quarantine/skill-*` ref exists, Skill paths
do not reach origin, and every non-Skill delta does. Assert hard-divergence reset
does not run while the quarantine journal is unresolved.

- [ ] **Step 2: Implement the guard facade**

```rust
pub struct SkillSyncGuard {
    checkpoint: SkillCheckpointStore,
}

pub enum IntegrationOperation {
    RebaseOntoOrigin,
    HardDivergenceRecovery,
    FollowEpochRedirect,
}

pub enum GuardedPushOutcome {
    Pushed,
    NothingToPush,
    RepairedAndPushed { quarantine_ref: String },
}

impl SkillSyncGuard {
    pub fn guarded_push(
        &self,
        repo: &GitStorage,
        commit_lock: &std::sync::Mutex<()>,
        author: (&str, &str),
    ) -> Result<GuardedPushOutcome, SkillSyncError>;

    pub fn guarded_integrate(
        &self,
        repo: &GitStorage,
        operation: IntegrationOperation,
    ) -> Result<IncomingSkillValidation, SkillSyncError>;

    pub fn rotation_allowed(
        &self,
        repo: &GitStorage,
    ) -> Result<(), SkillSyncError>;
}
```

Make raw working-branch push crate-private. The private Skill ref push and epoch
atomic push use explicit distinct methods. Bypass repair creates the quarantine
ref and journal first, replays commits with Skill pathspec exclusions, invokes
the existing thread/meta resolver for non-Skill conflicts, verifies all
non-Skill deltas plus accepted Skill tree OIDs, and moves the local branch only
after verification. Generic epoch `.thread` replay excludes both Skill roots.

User archive commits receive a `skills/` root-tree semantic precondition so a
concurrent role addition either wins and blocks archive or loses and sees an
inactive user.

- [ ] **Step 3: Route every call site**

Replace direct handler calls with `state.push_working_branch(...)`; extend
`start_sync_loop` with the shared guard; wrap pull-only, rebase retry, conflict
resolve, divergence cleanup, and redirect follow. Add a test-only assertion or
source audit that no daemon code calls raw `GitStorage::push`.

- [ ] **Step 4: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-sync --test skill_guard --locked
cargo test -p gitim-daemon --test skill_sync_safety --locked
cargo test -p gitim-sync --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 7 guarded sync diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 7 guarded sync diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add crates/gitim-sync crates/gitim-daemon
git commit -m "feat(sync): guard every skill-sensitive git transition" \
  -m "Test: cargo test -p gitim-sync --locked; cargo test -p gitim-daemon --test skill_sync_safety --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 8: Remote Skill transaction, CAS retry, and recovery

**Files:**

- Create: `crates/gitim-sync/src/skill/transaction.rs`
- Modify: `crates/gitim-sync/src/skill/mod.rs`
- Test: `crates/gitim-sync/tests/skill_transaction.rs`

**Interfaces:**

- Consumes: typed request, immutable package snapshot, actor/active-user facts,
  core mutation planner, private-index builder, checkpoint store.
- Produces: durable remote result and accepted read-view commit, including an
  administrator-authorized repair commit built from the checkpoint's accepted
  workspace or Skill tree.

- [ ] **Step 1: Write failing race and crash tests**

Use two clones against one bare remote. Cover same-Skill concurrent proposals,
unrelated message CAS loss and retry, semantic read-set change and stale result,
epoch rotation between attempts, and injected crashes after `prepared`, `built`,
and `pushed`. Assert IDs are generated once; a successful lost response returns
the original root receipt result; mismatched reuse is rejected globally. Cover
workspace- and Skill-scoped repair races: a changed conflict checkpoint or
remote accepted tree rejects the repair instead of restoring stale authority.
Inject a hanging Git child and a saturated semaphore using test-only duration
configuration; assert deadline termination, permit release, retained journal,
unchanged checkpoint, and retryable error classification.

- [ ] **Step 2: Implement transaction state**

```rust
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum SkillTransactionPhase {
    Prepared,
    Built,
    Pushed,
    Completed,
}

pub struct RemoteSkillTransactionRequest {
    pub request: SkillMutationRequest,
    pub actor: String,
    pub author_email: String,
    pub now: String,
    pub package: Option<ValidatedPackage>,
    pub active_users: BTreeSet<String>,
}

pub struct RemoteSkillTransactionResult {
    pub commit_id: String,
    pub result: SkillMutationResult,
    pub local_state: SkillLocalState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLocalState {
    Current,
    PendingSync,
}

pub fn execute_remote_skill_transaction(
    repo: &GitStorage,
    guard: &SkillSyncGuard,
    request: RemoteSkillTransactionRequest,
) -> Result<RemoteSkillTransactionResult, SkillSyncError>;
```

The coordinator snapshots source/IDs before network work, fetches and resolves
the active epoch on every attempt, reads the semantic path OIDs, checks the
workspace-global receipt path, plans from explicit remote-tip blobs, builds and
self-validates one commit, then normal-pushes the explicit commit/ref. It retries
at most three times only when every semantic OID is unchanged. Git/network work
runs under a four-permit per-workspace semaphore in bounded `spawn_blocking`.
The full transaction has a 180-second deadline; each Git child has a 60-second
deadline and is killed and waited on when it expires (the process group is
terminated where the platform supports it). Timeout errors release the permit,
increment the Skill transport-failure health counter, and remain retryable; a
timed-out attempt never advances the checkpoint or deletes its journal.

Expose the exact constants from `transaction.rs`:

```rust
pub const SKILL_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(180);
pub const SKILL_GIT_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);
pub const SKILL_GIT_MAX_CONCURRENCY: usize = 4;
```

Journal root is `.gitim/skill-transactions/<request-id>/`; startup recovery
searches the authoritative branch for the root receipt and leaves unreachable
objects for Git GC.

- [ ] **Step 3: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-sync --test skill_transaction --locked
cargo test -p gitim-sync --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 8 remote transaction diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 8 remote transaction diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add crates/gitim-sync
git commit -m "feat(sync): publish skill mutations with remote CAS" \
  -m "Test: cargo test -p gitim-sync --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 9: Daemon import, accepted reads, bootstrap, and v1a handlers

**Files:**

- Create: `crates/gitim-daemon/src/{skill_import.rs,skill_store.rs,skill_handlers.rs}`
- Modify: `crates/gitim-daemon/src/{lib.rs,api.rs,state.rs,main.rs}`
- Modify: `crates/gitim-daemon/src/handlers/{mod.rs,depart.rs,user.rs}`
- Modify: `crates/gitim-runtime/src/http.rs`
- Create: `crates/gitim-runtime/src/cli/cmd_skill_repair.rs`
- Modify: `crates/gitim-runtime/src/cli/{mod.rs,dto.rs,http.rs}`
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`
- Test: `crates/gitim-daemon/tests/{skill_handlers.rs,skill_sync_safety.rs}`
- Test: `crates/gitim-runtime/tests/skill_admin_repair.rs`

**Interfaces:**

- Consumes: core requests/DTOs and sync transaction/guard APIs.
- Produces: authoritative IPC operations and `skill_changed` events.

- [ ] **Step 1: Write failing handler tests**

Cover empty/paginated list, exact current and pinned archived load, resource
index bounds, binary output metadata, unpublished candidate rejection, package
directory race/symlink rejection, create/propose/publish/reject/withdraw,
read-after-push with `pending_sync`, Runtime workspace bootstrap, and synced
remote event invalidation. Add direct archive/depart tests for
administrator/Skill-role preconditions before phase one and at phase four.
Test new local and GitHub workspace creation plus startup recovery bootstrap.
Test `gitim-runtime repair-skill-state --workspace <slug> [--skill <slug>]
--conflict-tip <oid> --accepted-tree <oid> --confirm` rejects missing
confirmation, non-administrators, mismatched checkpoint values, and absent
conflicts; a valid repair preserves invalid commits, restores only the selected
accepted tree, creates a receipt, and resumes incoming validation without
exposing the rejected intermediate state.

- [ ] **Step 2: Implement import and read store**

```rust
pub fn snapshot_skill_directory(
    source: &Path,
    request_dir: &Path,
) -> Result<ValidatedPackage, SkillError>;

pub struct SkillReadView {
    pub accepted_commit: String,
    pub checkpoint: SkillValidationCheckpoint,
}

impl SkillStore {
    pub fn list(&self, query: SkillListQuery) -> Result<SkillListResponse, SkillError>;
    pub fn show(&self, query: SkillShowQuery) -> Result<SkillShowResponse, SkillError>;
    pub fn load(&self, reference: &SkillReference) -> Result<SkillLoadResponse, SkillError>;
    pub fn resource(&self, query: SkillResourceQuery) -> Result<SkillResourceResponse, SkillError>;
}
```

Use worktree files only when the relevant `HEAD` Skill tree OID equals the
checkpoint tree OID; otherwise use accepted commit blobs. Catalog cache key is
the accepted commit. `load` performs one recursive tree list and one
`SKILL.md` read.

- [ ] **Step 3: Implement IPC and bootstrap**

Add nested request variants for every v1a read/write operation and one
`Event::SkillChanged { slug, kind, event_revision, control_revision,
proposal_id, proposal_state_revision }`. Resolve actor through existing helpers,
reject departed actors, and map `SkillError` codes without string duplication.

`crates/gitim-runtime/src/http.rs::recover_single_workspace` invokes
`workspace_bootstrap` through the recovered human daemon, after
`provision_human` succeeds and before `recover_agents_for_workspace`, when
`skills/workspace.meta.yaml` is absent. Both new-workspace provisioning paths
invoke the same helper before `workspaces_create` reports the workspace ready.
It never writes Git directly.

Add a loopback Runtime endpoint used only by `cmd_skill_repair`; it resolves the
human handler, verifies that handler against the accepted
`WorkspaceSkillMeta.administrators`, and forwards a typed repair request to the
human daemon. The daemon compares `conflict_tip` and `accepted_tree` with the
locked local checkpoint before entering the normal remote transaction. The
agent-facing `gitim skill` tree and v1b public Skill HTTP routes do not expose
this operation.

- [ ] **Step 4: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-daemon --test skill_handlers --locked
cargo test -p gitim-daemon --test skill_sync_safety --locked
cargo test -p gitim-runtime --bin gitim-runtime --locked
cargo test -p gitim-runtime --test skill_admin_repair --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 9 daemon Skill vertical diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 9 daemon Skill vertical diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add crates/gitim-daemon crates/gitim-runtime
git commit -m "feat(daemon): expose shared skill lifecycle" \
  -m "Test: cargo test -p gitim-daemon --test skill_handlers --locked; cargo test -p gitim-daemon --test skill_sync_safety --locked; cargo test -p gitim-runtime --bin gitim-runtime --locked; cargo test -p gitim-runtime --test skill_admin_repair --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 10: Client and progressive CLI

**Files:**

- Modify: `crates/gitim-client/src/client.rs`
- Create: `crates/gitim-cli/src/commands/skill.rs`
- Modify: `crates/gitim-cli/src/commands/mod.rs`
- Modify: `crates/gitim-cli/src/main.rs`
- Test: `crates/gitim-cli/tests/skill_cli.rs`

**Interfaces:**

- Consumes: daemon IPC.
- Produces: task-oriented `gitim skill` command tree and stable JSON output.

- [ ] **Step 1: Write failing CLI tests**

Assert root help shows only the five groups from the design; nested help exposes
all commands; every write accepts/generates `--request-id`; load prints only
`SKILL.md` plus at most 256 resource descriptors; binary `resource` requires
`--output`; `--force` and `--create-dirs` are explicit; daemon error codes map to
the runtime CLI's established numeric taxonomy: `0` success, `1` local
configuration/protocol failure, `2` permanent structured domain rejection, and
`3` retryable connection/timeout failure. Do not reuse the existing generic
`OutputMode::print` all-errors-as-1 behavior for Skill commands.

- [ ] **Step 2: Add typed client methods**

```rust
pub async fn skill_list(&self, query: &SkillListQuery) -> Result<ApiResponse, ClientError>;
pub async fn skill_load(&self, reference: &str) -> Result<ApiResponse, ClientError>;
pub async fn skill_create(&self, request: &SkillCreateRequest) -> Result<ApiResponse, ClientError>;
pub async fn skill_propose(&self, request: &SkillProposeRequest) -> Result<ApiResponse, ClientError>;
pub async fn skill_proposal_transition(
    &self,
    request: &SkillProposalTransitionRequest,
) -> Result<ApiResponse, ClientError>;
```

Add corresponding bounded read methods without putting domain logic in the
client.

- [ ] **Step 3: Implement nested clap commands and request journal**

`commands/skill.rs` owns formatting and
`.gitim/request-journal/<request-id>.json`. It writes the journal atomically
before dispatch, reuses a sole matching pending request, refuses a target or
fingerprint mismatch, and removes the entry only after definitive success or
domain failure. Its exhaustive `SkillCliExit` mapping mirrors
`crates/gitim-runtime/src/cli/exit_code.rs`: successful response `0`;
`ProtocolError`/local journal/config failures `1`; daemon `ok:false` with a
typed `SkillError` code `2`; `ConnectionFailed`, `Timeout`, and daemon-not-ready
transport failures `3`. Tests lock every `ClientError` and `SkillError` variant
to one class so later additions cannot silently fall through.

- [ ] **Step 4: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-client --locked
cargo test -p gitim-cli --test skill_cli --locked
cargo test -p gitim-cli --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 10 Skill CLI diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 10 Skill CLI diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add crates/gitim-client crates/gitim-cli
git commit -m "feat(cli): add shared skill commands" \
  -m "Test: cargo test -p gitim-client --locked; cargo test -p gitim-cli --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 11: Minimal provider contract and pinned-load end-to-end

**Files:**

- Modify: `crates/gitim-agent-provider/src/prompts.rs`
- Modify: `crates/gitim-agent-provider/tests/prompt_test.rs`
- Create: `crates/gitim-runtime/tests/skill_agent_load.rs`

**Interfaces:**

- Consumes: working CLI load surface.
- Produces: bounded agent awareness and verified exact-revision loading.

- [ ] **Step 1: Add failing prompt and runtime tests**

Prompt test asserts the four-line contract contains `skill:<slug>@<revision>`,
`gitim skill load <ref>`, and `gitim skill --help`; it rejects injected Skill
names, descriptions, bodies, catalogs, role rules, and full command lists.

Runtime test seeds a valid published fixture, sends a message containing its
pinned ref to a mock provider with a shell-tool transcript, and asserts the
provider invokes `gitim skill load` with the exact revision before producing the
answer. The test also asserts no runtime-native Skill path is created.

- [ ] **Step 2: Add the stable prompt entry**

Append exactly:

```text
GitIM provides optional shared Skills and does not load them automatically.
When a message contains skill:<slug>@<revision>, run gitim skill load <ref> before handling the related task.
When the user asks to discover, sediment, or maintain a Skill, run gitim skill --help instead of guessing commands.
Loading a Skill does not grant permission to execute its scripts.
```

- [ ] **Step 3: Verify, review, and commit**

Run:

```bash
cargo test -p gitim-agent-provider --test prompt_test --locked
cargo test -p gitim-runtime --test skill_agent_load --locked
```

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the current uncommitted Task 11 agent loading diff. Do not modify files. Report only correctness, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the current uncommitted Task 11 agent loading diff. Do not modify files. Report only correctness, compatibility, or test gaps." \
  --output-format text
```

Fold accepted corrections, rerun the verification commands, then:

```bash
git add crates/gitim-agent-provider crates/gitim-runtime
git commit -m "feat(runtime): load pinned shared skills explicitly" \
  -m "Test: cargo test -p gitim-agent-provider --test prompt_test --locked; cargo test -p gitim-runtime --test skill_agent_load --locked" \
  -m "Co-authored-by: Codex <codex@openai.com>"
```

### Task 12: v1a acceptance and mergeability gate

**Files:**

- Modify generated wasm output if core exports changed after Task 4.
- Modify only failing implementation/tests discovered by verification.

**Interfaces:**

- Consumes: all previous tasks.
- Produces: a mergeable v1a branch with deterministic evidence.

- [ ] **Step 1: Run the scoped safety matrix**

```bash
cargo test -p gitim-core --locked
cargo test -p gitim-sync --locked
cargo test -p gitim-daemon --test skill_handlers --locked
cargo test -p gitim-daemon --test skill_sync_safety --locked
cargo test -p gitim-client --locked
cargo test -p gitim-cli --locked
cargo test -p gitim-agent-provider --test prompt_test --locked
cargo test -p gitim-runtime --test skill_agent_load --locked
cargo test -p gitim-runtime --test skill_admin_repair --locked
```

Expected: all pass.

- [ ] **Step 2: Run repository quality gates**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked
cargo test --locked
cd products/gitim/frontend
npm run build:wasm
npm run lint
npm run build
cd /Users/lewisliu/ateam/GitIM/.codex/skill-support
git diff --check
```

Expected: all pass. Full `cargo test --locked` is required because v1a changes a
workspace dependency and shared protocol behavior.

- [ ] **Step 3: Run final Cursor and Kimi reviews**

Run:

```bash
/Users/lewisliu/.local/bin/cursor-agent --print --output-format stream-json --yolo \
  --workspace /Users/lewisliu/ateam/GitIM/.codex/skill-support \
  --model cursor-grok-4.5-high \
  "Review the complete codex/skill-support v1a diff against docs/plans/skill-support/00-requirements.md and docs/plans/skill-support/01-v1a-implementation-plan.md. Do not modify files. Report only merge-blocking correctness, security, recovery, compatibility, or test gaps."
/Users/lewisliu/.kimi-code/bin/kimi --model kimi-code/k3 \
  --prompt "Review the complete codex/skill-support v1a diff against docs/plans/skill-support/00-requirements.md and docs/plans/skill-support/01-v1a-implementation-plan.md. Do not modify files. Report only merge-blocking correctness, security, recovery, compatibility, or test gaps." \
  --output-format text
```

Fold every accepted finding and repeat Steps 1–2.

- [ ] **Step 4: Inspect final history and status**

```bash
git log --oneline --decorate d83c61c3..HEAD
git status --short
git diff d83c61c3...HEAD --stat
```

Expected: conventional commits with required trailers; no unexpected files or
uncommitted implementation changes.

- [ ] **Step 5: Ask the user at the mergeable boundary**

Report test evidence, external review status, commit range, and remaining v1b/v1c
scope. Ask only whether to merge/push this v1a slice or continue accumulating
later slices in the same branch.
