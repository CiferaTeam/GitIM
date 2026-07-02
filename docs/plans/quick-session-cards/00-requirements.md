# Quick Session Cards Requirements

Status: Draft
Date: 2026-07-02

## Problem

GitIM already has durable channels, DMs, task cards, and long-running agent sessions. Some user work is smaller than a task card but still needs an isolated thread: a quick question, a short investigation, a draft prompt, or a lightweight back-and-forth with one or more agents over time.

The current long agent conversation model makes these items feel scattered, while placing them inside task cards makes the feature feel heavier than the job. Quick Session Cards add a top-level lightweight conversation layer that can be created quickly, archived quickly, and referenced into durable GitIM spaces when it becomes useful.

## Goals

1. Provide a top-level quick conversation hub near the main navigation tabs.
2. Let users create a quick session with an agent picker and first message only.
3. Generate a default title automatically from the first message, then refine it asynchronously when possible.
4. Persist quick sessions as addressable GitIM objects with stable refs.
5. Give every quick session its own provider session token, usage state, compression lifecycle, and transcript.
6. Preserve agent identity by inheriting the selected agent's provider, model, system prompt, environment, Hermes profile, and repo context.
7. Scope runtime events by session so each frontend session panel receives only its own stream.
8. Support archiving from the quick session hub.
9. Support dragging a quick session into a channel or card conversation to insert a reference.

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
- Status: active, running, archived, or error.
- Stable ref, displayed as a compact token.
- Updated time.

Creation flow:

1. User opens the hub.
2. User selects an agent or accepts the default recent agent.
3. User types the first message.
4. Frontend creates the quick session and immediately opens its mini panel.
5. Backend stores a provisional title derived from the first message.
6. Runtime starts the selected agent turn and may later replace the title with a generated title.

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
- `title_source`: `first_message` or `generated`
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
- Serialize all turns for the same agent through a per-agent work queue.
- Emit scoped activity events for each quick session.
- Update title, summary, status, and usage snapshots.

Quick session turns inherit the selected agent's durable identity and config. The selected agent's main long-session provider token remains reserved for the primary agent session.

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
2. The session receives a generated title from the first-message flow.
3. Multiple quick sessions using the same agent each maintain independent frontend event state.
4. The selected agent's main activity panel displays only main-session streaming output.
5. Main agent state and quick session state have separate provider session tokens.
6. Archiving removes a quick session from the active hub list and preserves ref resolution.
7. Dragging a quick session into a channel/card composer inserts a stable ref.
8. Reference preview resolves quick session refs.
9. Scoped backend tests prove state isolation, event scoping, and compression behavior.
10. Frontend tests cover create, stream, archive, drag-ref, and event routing.
