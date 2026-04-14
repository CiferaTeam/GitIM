# Frontend Incremental Data Flow

## Problem

Current webui-v2 frontend has three data flow issues:

1. **No state persistence** — refresh loses everything, poll cursor (commit_id) gone, all channels re-fetch from scratch
2. **Redundant full reads** — poll returns complete message entries, but frontend ignores them and does a full `read()` on the active channel anyway
3. **No read limit** — `read(channel)` called without `limit`, pulls entire channel history every time

## Design

### Anchor: commit_id

The single persistent anchor between frontend and backend is the poll cursor (`commit_id`). Stored in localStorage, namespaced by workspace.

### Backend Changes

**1. Health endpoint returns workspace path**

`GET /health` response adds `workspace` field:

```json
{
  "service": "gitim-runtime",
  "version": "0.3.1",
  "initialized": true,
  "workspace": "/Users/lewis/my-workspace"
}
```

Frontend converts this to a localStorage namespace key by replacing `/` with `-`:

```
localStorage key: "gitim:cursor:-Users-lewis-my-workspace"
```

No other backend changes needed. Existing APIs are sufficient:
- `read(channel, limit, since)` — already supports limit and line-based filtering
- `poll(since)` — already returns full message entries, not just notifications

### Frontend Changes

**1. commit_id persistence (localStorage)**

```
On app init:
  1. GET /health → extract workspace path → build storage key
  2. Read commit_id from localStorage[storage_key]
  3. poll(since=commit_id) → get changes since last session
  4. Store new commit_id back to localStorage[storage_key]

On each poll cycle:
  1. poll(since=commit_id) → changes
  2. Update localStorage[storage_key] = new commit_id
```

**2. Poll data direct append (remove redundant read)**

Current:
```
poll → changes for active channel → client.read(channel) → setMessages(full)
```

New:
```
poll → changes for active channel → addMessages(change.entries)
```

Poll entries and read entries share the same ThreadEntry structure. No transformation needed.

**3. Enter channel: read with limit**

Current:
```
selectChannel → client.read(channel) → setMessages(all entries)
```

New:
```
selectChannel → client.read(channel, limit=50) → setMessages(last 50 entries)
```

**4. New message positioning**

When user opens the app after being away:

```
1. poll(since=stored_commit_id) → changes grouped by channel
2. Sidebar: each channel shows unread count from poll changes
3. User clicks channel:
   a. read(channel, limit=50) → load last 50 messages
   b. Scroll to first message that appeared in poll changes (first unread)
```

Determining "first unread" position: poll changes include entries with `line_number`. The minimum `line_number` from poll changes for that channel is the scroll target.

**5. Error recovery: stale commit_id**

If `poll(since=old_commit_id)` returns an error (commit no longer exists):

```
1. Discard stored commit_id
2. poll(since=null) → get current commit_id as new baseline
3. Store new commit_id
4. Treat all channels as "no unread" (fresh start)
```

### Not In Scope

- `before` / `offset` parameter on read API (no backward pagination)
- Adaptive poll interval
- Per-channel read position tracking (server-side or client-side)
- Channel list incremental update via poll (keep existing full refresh on unknown channel)
