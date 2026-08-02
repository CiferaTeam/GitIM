# GitIM Shared Skills — Event Protocol

Status: implemented

## Goal

GitIM workspaces can publish, discover, load, and collaboratively improve shared Agent Skills packages through an explicit capability layer isolated from each AI runtime's native Skill system.

## Product contract

- A shared Skill coexists with Codex, Claude, Cursor, Gemini, Hermes, OpenCode, and other runtime-native Skills as an isolated GitIM capability layer.
- Agents and humans discover and manage shared Skills explicitly through `gitim skill ...`.
- A message reference uses `skill:<slug>@<revision-id>`. A pinned reference always loads the same immutable package revision.
- The frontend may later use `/` discovery to insert a pinned reference; `/` is a UI gesture and is not part of the stored reference.
- Loading returns instructions and an index. Script execution remains a separate, explicit provider tool action.
- Git commits remain the workspace audit and transport mechanism.

## Package format

Each revision stores an Agent Skills-compatible package:

```text
SKILL.md
scripts/
references/
assets/
```

`SKILL.md` is required and starts with YAML frontmatter containing `name` and `description`. `name` must equal the GitIM Skill slug.

Limits:

- `SKILL.md`: 64 KiB
- One resource: 5 MiB
- Complete package: 10 MiB
- Files per package: 256
- Symlinks and special files: rejected
- Paths: portable relative UTF-8 paths only; no traversal, absolute paths, backslashes, empty components, hidden control files, or ASCII case-insensitive collisions

Package identity is the SHA-256 of a canonical, path-sorted manifest containing each path, byte length, and file bytes.

## Repository layout

```text
skills/<slug>/
├── revisions/
│   └── r-<ulid>/
│       ├── revision.meta.yaml
│       └── package/
│           └── ...
└── events/
    └── e-<ulid>.meta.yaml
```

Current state is derived exclusively from immutable revision and event records. The existing GitIM sync loop transports their normal commits.

`revision.meta.yaml` is immutable:

```yaml
schema_version: 1
id: r-01...
skill: release-check
base_revision: r-01... # null for the initial revision
content_sha256: 0123...
resources: []
created_by: alice
created_at: "2026-08-02T10:00:00Z"
```

Every lifecycle mutation creates one immutable event:

```yaml
schema_version: 1
id: e-01...
skill: release-check
actor: alice
created_at: "2026-08-02T10:00:00Z"
type: created
display_name: Release Check
description: Verify a release candidate.
revision: r-01...
```

Supported event types:

- `created`
- `proposal_opened`
- `proposal_commented`
- `proposal_published`
- `proposal_rejected`
- `proposal_withdrawn`
- `metadata_updated`
- `owner_added` / `owner_removed`
- `maintainer_added` / `maintainer_removed`
- `archived` / `unarchived`

Proposal IDs use `p-<ulid>`. Proposal state is the effective `proposal_opened` event plus later events that reference its ID.

## Deterministic reduction

Current state is derived by a pure reducer:

1. Parse event files and require the file name, embedded event ID, and embedded slug to agree.
2. Sort by event ID.
3. Apply events in that order.
4. Record a well-formed event whose precondition no longer holds as `ineffective`; keep it in history without changing state.
5. Reject a malformed event stream for that Skill only. Other Skills remain usable.

The daemon creates every new event ID after the greatest event ID it has observed. Concurrent writers may both extend the same observed history; their unique paths merge through normal Git sync, and event-ID ordering supplies a stable tie-breaker. Once merged, later event IDs are generated after both branches, so accepted history does not reorder again.

Reducer rules:

- `created` is effective only once and makes the creator the initial owner and maintainer.
- A proposal references an immutable candidate revision and a published base revision.
- Concurrent proposals are independent.
- Publish is effective only when the actor is a maintainer, the proposal is open, and `expected_current_revision`, proposal base, and current revision agree.
- Competing publishes are all retained; the first effective event wins and later stale events become ineffective.
- Reject requires a maintainer. Withdraw requires the proposal author.
- Metadata updates require a maintainer.
- Owner and maintainer changes require an owner. An owner is always a maintainer, and the final owner cannot be removed.
- Archive and unarchive require an owner. Archived Skills reject unpinned loads and content mutations; pinned published revisions remain loadable.
- Proposal candidate revisions are not available through normal `skill:` loads until an effective publish event references them.

An optional caller-supplied event ID is the idempotency key. Reusing it with the same actor and semantic payload returns the existing result; reusing it with different content returns `skill_event_conflict`.

## Permissions

| Operation | Active member | Proposal author | Maintainer | Owner |
|---|---:|---:|---:|---:|
| List/show/load published content | Yes | Yes | Yes | Yes |
| Create a Skill | Yes | Yes | Yes | Yes |
| Open or comment on a proposal | Yes | Yes | Yes | Yes |
| Withdraw an open proposal | No | Yes | If author | If author |
| Reject or publish an open proposal | No | No | Yes | Yes |
| Change display metadata | No | No | Yes | Yes |
| Change maintainers or owners | No | No | No | Yes |
| Archive or unarchive | No | No | No | Yes |

The daemon verifies that actors and new role targets are active workspace members when accepting a write. Permissions govern protocol-correct collaboration among workspace members; Git history supplies the audit trail.

## CLI surface

The root help is task-oriented. Detailed options remain in nested help instead of the provider prompt.

```text
gitim skill list [--archived] [--limit N] [--after SLUG]
gitim skill show <slug>
gitim skill load <skill-ref-or-slug[@revision]>
gitim skill resource <skill-ref-or-slug[@revision]> <path> [--output PATH] [--force]
gitim skill ref <slug> [--revision REVISION]
gitim skill revisions <slug> [--limit N] [--after REVISION]
gitim skill history <slug> [--limit N] [--after EVENT]
gitim skill validate --from DIRECTORY

gitim skill create <slug> --from DIRECTORY --display-name NAME --description TEXT
gitim skill propose <slug> --from DIRECTORY --base REVISION --summary TEXT

gitim skill proposal list <slug> [--status STATUS] [--limit N] [--after PROPOSAL]
gitim skill proposal show <slug> <proposal>
gitim skill proposal resource <slug> <proposal> <path> [--output PATH] [--force]
gitim skill proposal comment <slug> <proposal> --body TEXT
gitim skill proposal publish <slug> <proposal>
gitim skill proposal reject <slug> <proposal>
gitim skill proposal withdraw <slug> <proposal>

gitim skill admin update <slug> [--display-name NAME] [--description TEXT]
gitim skill admin archive <slug>
gitim skill admin unarchive <slug>
gitim skill role owner-add <slug> <handler>
gitim skill role owner-remove <slug> <handler> [--remove-maintainer]
gitim skill role maintainer-add <slug> <handler>
gitim skill role maintainer-remove <slug> <handler>
```

Every write accepts `--event-id`. JSON mode is available through the existing global `--json` flag.

Bounds:

- Lists default to 50 and cap at 100.
- Proposal lists omit comment bodies; proposal detail returns the latest 100 comments and a truncation flag.
- Load returns `SKILL.md` plus resource metadata, not resource bodies.
- Text resources may print to stdout. Binary resources require `--output`.
- Display name: 1–80 characters.
- Description: 1–1024 characters.
- Proposal summary: 1–500 characters.
- Proposal comment: 1–10,000 characters.

## Provider contract

The base prompt contains only:

```text
GitIM shared Skills are optional and are not loaded automatically.
When a message contains skill:<slug>@<revision>, run `gitim skill load <ref>` before using it.
For discovery or management, run `gitim skill --help` and follow nested help.
Loading a Skill does not authorize executing its scripts.
```

Catalog entries, package bodies, role details, and nested command help are fetched explicitly when needed.

## Error behavior

Stable codes include:

- `skill_not_found`, `skill_exists`, `skill_archived`
- `skill_invalid_slug`, `skill_invalid_ref`, `skill_invalid_package`, `skill_package_too_large`
- `skill_invalid_input`
- `skill_revision_not_found`, `skill_revision_unpublished`, `skill_revision_corrupted`
- `skill_proposal_not_found`, `skill_proposal_terminal`
- `skill_not_maintainer`, `skill_not_owner`, `skill_last_owner`, `skill_owner_is_maintainer`
- `skill_role_target_inactive`, `skill_invalid_history`, `skill_event_conflict`
- `skill_resource_not_found`, `skill_resource_binary`, `output_exists`

Invalid event or revision metadata blocks only the affected Skill's show/load/write operations. Catalog responses include a bounded `invalid` list so corruption is visible without blocking healthy Skills. Package bytes are hash-checked on load/resource and return `skill_revision_corrupted` without forcing catalog scans to read every historical package.

## Delivery boundary

This slice delivers:

- Core IDs, refs, package validation, event schema, and reducer
- Daemon storage, reads, writes, permissions, event history, and normal Git commits
- Typed client methods and complete agent-facing CLI
- Minimal provider prompt contract
- Tests covering deterministic convergence, permissions, immutable package loading, CLI behavior, and normal sync compatibility

A follow-up slice owns bounded Runtime read APIs, frontend slash discovery, Skill chips, and browser management.
