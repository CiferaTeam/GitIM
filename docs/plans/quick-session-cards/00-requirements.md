# Quick Session Cards Requirements

Status: Draft
Date: 2026-07-02

## Problem

GitIM already has durable channels, DMs, task cards, and long-running agent sessions. Some user work is smaller than a task card but still needs an isolated thread: a quick question, a short investigation, a draft prompt, or a lightweight back-and-forth with one or more agents over time.

The current long agent conversation model makes these items feel scattered, while placing them inside task cards makes the feature feel heavier than the job. Quick Session Cards add a top-level lightweight conversation layer that can be created quickly, archived quickly, and referenced into durable GitIM spaces when it becomes useful.

## Goals

1. Provide a top-level quick conversation hub near the main navigation tabs.
2. Let users create a quick session with an agent picker and first message only.
3. Require the executing agent to set a session title via an API gate before its first assistant response; reject the turn if title is not set.
4. Support cross-node quick sessions: create and read from any node via GitIM git-synced card/thread protocol; agent execution runs on the node that owns the selected agent.
5. Persist quick sessions as addressable GitIM objects with stable refs.
6. Give every quick session its own provider session token, usage state, compression lifecycle, and transcript.
7. Preserve agent identity by inheriting the selected agent's provider, model, system prompt, environment, Hermes profile, and repo context.
8. Scope runtime events by session so each frontend session panel receives only its own stream.
9. Support archiving from the quick session hub.
10. Support dragging a quick session into a channel or card conversation to insert a reference.

## Product Contract

The quick session hub lives one level above the current channel/card workspace. It is a compact floating surface opened from the top navigation area by hover and stabilized by click/focus while the user interacts with it.

The hub has three areas:

1. Active quick session list, aggregated across agents.
2. Create composer with agent picker and message input.
3. Mini conversation panel for the selected quick session.

Each list item shows:

- Auto title.
- Agent identity.
- Short latest-message preview.
- Status: needs_title, active, running, archived, or error.
- Stable ref, displayed as a compact token.
- Updated time.

Creation flow:

1. User opens the hub.
2. User selects an agent or accepts the default recent agent.
3. User types the first message.
4. Frontend creates the quick session with `status = needs_title` and opens its mini panel.
5. Runtime starts the selected agent turn. The agent must call `set_quick_session_title(session_id, title)` before sending any assistant content.
6. If the agent sends assistant content without setting a title, the runtime returns a typed error and rejects the turn.
7. After title is set, agent resumes normal reply flow. UI replaces the placeholder with the title.
8. Title source is recorded as `api_set`. The title may be updated by later `set_quick_session_title` calls (e.g., if the agent refines it mid-session).

Archive flow:

1. User clicks archive in the session row or mini panel.
2. Session leaves the active list.
3. Archived sessions remain addressable by ref and can appear in search/reference previews.

Reference flow:

1. User drags a quick session row or ref token into a channel/card composer.
2. Frontend inserts the session ref into the target composer.
3. Sending the target message persists the ref as normal message content.
4. Reference preview resolves the quick session title, agent, status, and latest summary.

## Data Contract

Quick sessions are first-class GitIM objects with runtime-owned execution state.

Persisted GitIM files:

```text
quick-sessions/<quick-session-id>/session.meta.yaml
quick-sessions/<quick-session-id>/discussion.thread
```

Runtime-local state:

```text
<workspace>/.gitim-runtime/quick-sessions/<quick-session-id>.state.json
```

`session.meta.yaml` stores durable metadata:

- `id`
- `title`
- `title_source`: `api_set` (set by agent via title API gate) or `none` (before title is set)
- `agent_id`
- `created_by`
- `status`
- `created_at`
- `updated_at`
- `archived_at`
- `ref`
- `summary`
- `last_message_preview`

`discussion.thread` stores the visible quick session transcript using the existing GitIM line/thread format.

The runtime state stores provider execution details:

- `session_token`
- `session_usage`
- `estimated_tokens`
- `last_session_usage`
- `usage_notice_pending`
- `last_compaction_at`

## Reference Contract

The stable ref returned by create/read APIs is:

```text
session:<quick-session-id>
```

`gitim-core::link` should recognize this as a quick session reference. Frontend message parsing and reference preview should resolve it through the quick session read API.

## Backend Contract

Daemon owns persisted quick session object writes and validation:

- Create quick session.
- Append user/agent messages.
- List active and archived quick sessions.
- Read quick session metadata and transcript.
- Archive or unarchive quick session.
- Resolve quick session ref.

Runtime owns provider execution:

- Build provider config from the selected agent.
- Run the quick session turn with its own state file.
- Enforce the title API gate: require `set_quick_session_title` before sending assistant content.
- Serialize all turns for the same agent through a per-agent work queue.
- Emit scoped activity events for each quick session.
- Update title, summary, status, and usage snapshots.

Quick session turns inherit the selected agent's durable identity and config. The selected agent's main long-session provider token remains reserved for the primary agent session.

## Cross-Node Routing Contract

Quick sessions support cross-node creation and routing through the existing GitIM git-synced card/thread protocol. This enables a human on one node to create a quick session that targets an agent on another node.

**What crosses nodes (git-synced):**
- `session.meta.yaml` and `discussion.thread` — persisted by daemon, committed and pushed like any other GitIM object (card, channel, DM).
- Any node can create, read, list, or archive a quick session by syncing these files through git.

**What stays local (runtime-bound):**
- Provider session token, streaming state, compaction state, and `.gitim-runtime/quick-sessions/<id>.state.json` — these live only on the runtime that owns the selected agent.
- Provider hidden session state (cursor, in-flight process state) does not cross nodes.

**Execution routing:**
- A quick session is executed by the runtime on the node where the selected agent runs.
- When a daemon detects a new quick session (via git poll) targeting an agent it hosts, it dispatches the turn to its local runtime.
- The initiating node polls for git updates to receive the agent's response in `discussion.thread`.

**Event delivery:**
- Scoped activity events are delivered through existing poll/SSE channels to the daemon that initiated the session.
- The initiating frontend consumes quick session events the same way regardless of which node executes the turn.

**Constraints:**
- V1 does not support provider session continuity across nodes (no "handoff" of a running provider process from one machine to another).
- If the agent's node is offline, the quick session remains in `needs_title` or `active` status until the node comes online and processes the turn.

## Event Contract

`AgentActivityEvent` gains optional scoping fields:

- `scope`: `agent_main` or `quick_session`
- `session_id`
- `ref`

Existing consumers that only care about main agent activity continue to treat missing scope as `agent_main`.

Quick session UI only applies events where `scope == "quick_session"` and `session_id` matches the selected quick session. Usage events for quick sessions patch quick session state while still allowing workspace-level usage totals to aggregate by agent/provider.

## API Contract

Runtime HTTP endpoints:

- `POST /workspaces/{slug}/quick-sessions`
- `GET /workspaces/{slug}/quick-sessions?archived=false`
- `GET /workspaces/{slug}/quick-sessions/{id}`
- `POST /workspaces/{slug}/quick-sessions/{id}/messages`
- `POST /workspaces/{slug}/quick-sessions/{id}/title` — agent sets the session title; must be called before first assistant message
- `PATCH /workspaces/{slug}/quick-sessions/{id}`
- `POST /workspaces/{slug}/quick-sessions/{id}/archive`
- `POST /workspaces/{slug}/quick-sessions/{id}/unarchive`

The existing runtime agent activity stream carries scoped quick session events after the schema change.

Daemon IPC/client operations mirror the persisted-object API:

- `create_quick_session`
- `list_quick_sessions`
- `read_quick_session`
- `send_quick_session_message`
- `update_quick_session`
- `archive_quick_session`
- `unarchive_quick_session`

## Compression Contract

Each quick session has its own compression state. Token estimates, usage notices, session reset markers, and summaries apply only to that quick session.

When a quick session is compacted or reset:

1. Runtime writes a summary back to quick session metadata.
2. Runtime clears the quick session provider token.
3. The next turn restores context from durable transcript plus summary.
4. The selected agent's main session state remains unchanged.

## UX Boundaries

Quick sessions are lighter than task cards. They support conversation, archive, title, status, usage, and references. Task cards continue to own workflow fields such as assignee transitions, checklist-like progress, and board movement.

Quick sessions are top-level user objects. Each turn executes through one selected agent, while the object remains visible from the global quick session hub.

## Acceptance Criteria

1. A user can create a quick session from the top hub with only an agent and first message.
2. The session enforces a title API gate: agent must call `set_quick_session_title` before sending assistant content; sending assistant content without a title returns a typed error.
3. Multiple quick sessions using the same agent each maintain independent frontend event state.
4. The selected agent's main activity panel displays only main-session streaming output.
5. Main agent state and quick session state have separate provider session tokens.
6. Archiving removes a quick session from the active hub list and preserves ref resolution.
7. Dragging a quick session into a channel/card composer inserts a stable ref.
8. Reference preview resolves quick session refs.
9. Scoped backend tests prove state isolation, event scoping, and compression behavior.
10. Frontend tests cover create, stream, archive, drag-ref, and event routing.
11. A quick session targeting an agent on another node is created, executed by that agent's runtime, and the response is readable from the initiating node.
12. Scoped provider tests prove cross-node daemon sync: a session created on node A is detected and executed by agent runtime on node B.
