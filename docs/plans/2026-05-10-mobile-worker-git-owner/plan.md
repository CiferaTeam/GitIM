# Mobile Worker Git Owner Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Prevent mobile/browser local mode from blanking the workspace when Worker sync observes remote changes, then make Worker sync the single Git state owner for local polling.

**Architecture:** First, replace App-side `sync_reset` store clearing with an active-workspace revalidate path that keeps old UI visible until fresh data is ready. Then, return structured `SyncResult` values from `daemon-web/sync.ts` and make `handlers.poll` call `runSync()` instead of doing its own `fetch/reset` sequence.

**Tech Stack:** React 19, Zustand, TypeScript, Web Worker RPC, isomorphic-git, LightningFS, Vitest.

---

## Engineering Review Notes

- The fix must not rely on interval jitter. The bug is a state ownership problem, not just timing.
- `sync_reset` is the direct blank-screen trigger because App currently calls all workspace reset functions from the event callback. That path must stop clearing stores.
- `handlers.poll` currently owns a second Git fetch/reset path. It must reuse Worker sync so App poll and background sync cannot race over the same repository.
- The App reload path must be reusable by init and repo-change events; duplicating bootstrap fetch code in an event callback is too fragile.
- Existing send behavior depends on `runSync({ forceNewCycle: true })` surfacing failures to `syncAfterCommit`; preserving thrown errors for failed sync is required.

## File Structure

- Modify: `products/gitim/frontend/src/app.tsx`
  - Extract active workspace bootstrap/revalidate into a reusable callback.
  - Change local Worker repo-change callback to revalidate instead of clearing stores.
  - Handle `poll(...).data.reset === true` with cursor reset plus revalidate.
- Modify: `products/gitim/frontend/src/lib/types.ts`
  - Add optional `reset?: boolean` to `PollChange` response typing surface via local cast expectations.
- Modify: `products/gitim/frontend/src/daemon-web/sync.ts`
  - Add `SyncResult`.
  - Make `runSync()` return structured success results while still throwing on failed sync.
  - Emit `repo_changed` for remote fast-forward/rebase events and `sync_error` / `reconnect_required` for failed background sync.
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
  - Extend `WorkerEvent` with `repo_changed` and `reconnect_required`.
  - Scope the new events the same way `sync_reset` is scoped.
- Modify: `products/gitim/frontend/src/lib/backend.ts`
  - Treat `repo_changed` as the compatibility successor of `sync_reset`.
  - Clear the current browser token on `reconnect_required`.
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
  - Make `poll(since)` call `runSync()`.
  - Remove direct `fetchOrigin` / `resolveRemoteHead` / `resetToRemote` from poll.
  - Return `{ reset: true }` for stale cursor diff failures.
- Modify: `products/gitim/frontend/src/lib/backend.test.ts`
  - Cover scoped `repo_changed` events and reconnect-required token clearing.
- Modify: `products/gitim/frontend/src/daemon-web/sync.test.ts`
  - Cover `SyncResult` statuses, `repo_changed`, and concurrent `runSync()` deduplication.
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`
  - Cover local poll delegation to `runSync()` and stale cursor reset responses.
- Modify: `docs/plans/2026-05-10-mobile-worker-git-owner/design.md`
  - Mark the design as approved after implementation review.

## Task 1: App Revalidates On Repo Change

**Files:**
- Modify: `products/gitim/frontend/src/app.tsx`

- [x] **Step 1: Add message reload and channel selection store actions in App**

Add selectors next to existing chat store selectors:

```ts
const setMessages = useChatStore((s) => s.setMessages);
const selectChannel = useChatStore((s) => s.selectChannel);
```

- [x] **Step 2: Extract active workspace data loader**

Create a `reloadActiveWorkspaceState` callback before `runPoll`:

```ts
const reloadActiveWorkspaceState = useCallback(
  async (
    slug: string,
    workspaceKey: string,
    options: { preserveSelection: boolean },
  ): Promise<boolean> => {
    const isCurrentTarget = () =>
      slug === activeSlugRef.current && workspaceKey === workspaceRef.current;
    const previousChannel = useChatStore.getState().currentChannel;

    const [meRes, channelsRes, usersRes, agentsRes, cardsRes, boardsRes] =
      await Promise.all([
        client.me(slug),
        client.channels(slug),
        client.users(slug),
        mode === "remote"
          ? client.listAgents(slug)
          : Promise.resolve({ ok: true, data: { agents: [] } }),
        client.listCards(slug),
        client.listBoards(slug),
      ]);

    if (!isCurrentTarget()) return false;
    if (
      [meRes, channelsRes, usersRes, agentsRes, cardsRes, boardsRes].some(
        isUnknownWorkspaceResponse,
      )
    ) {
      await refreshAfterActiveUnavailable(slug);
      return false;
    }

    const nextChannels = channelsRes.ok && channelsRes.data
      ? (channelsRes.data.channels as Channel[])
      : useChatStore.getState().channels;

    let nextChannel: string | null = null;
    if (options.preserveSelection && previousChannel) {
      const archived = [
        ...useChatStore.getState().archivedChannels,
        ...useChatStore.getState().archivedDms,
      ];
      const stillVisible =
        nextChannels.some((c) => c.name === previousChannel) ||
        archived.some((c) => c.name === previousChannel);
      if (stillVisible) nextChannel = previousChannel;
    }
    if (!nextChannel) {
      nextChannel = nextChannels.find((c) => c.name === "general")?.name ?? null;
    }

    let messagesForChannel: Message[] | null = null;
    if (nextChannel) {
      const readRes = await client.read(slug, toApiChannel(nextChannel), 50);
      if (!isCurrentTarget()) return false;
      if (readRes.ok && readRes.data) {
        messagesForChannel = readRes.data.entries as Message[];
      }
    }

    if (meRes.ok && meRes.data) setCurrentUser(meRes.data.handler as string);
    if (channelsRes.ok && channelsRes.data) setChannels(nextChannels);
    if (usersRes.ok && usersRes.data) setUsers(usersRes.data.users as string[]);
    if (agentsRes.ok && agentsRes.data) setAgents(agentsRes.data.agents as Agent[]);
    if (cardsRes.ok && cardsRes.data) {
      const cards = cardsRes.data.cards as Card[];
      if (options.preserveSelection) mergeCards(cards);
      else setCards(cards);
    }
    if (boardsRes.ok && boardsRes.data) setBoards(boardsRes.data.boards as BoardSummary[]);

    if (nextChannel && nextChannel !== previousChannel) {
      selectChannel(nextChannel);
    }
    if (messagesForChannel) {
      useChatStore.getState().setMessages(messagesForChannel);
    }
    return meRes.ok && channelsRes.ok && usersRes.ok && agentsRes.ok && cardsRes.ok && boardsRes.ok;
  },
  [
    mode,
    refreshAfterActiveUnavailable,
    setCurrentUser,
    setChannels,
    setUsers,
    setAgents,
    setCards,
    setBoards,
    selectChannel,
  ],
);
```

- [x] **Step 3: Replace init bootstrap fetches with the loader**

In the `init(slug)` function, set:

```ts
workspaceRef.current = workspaceKey;
sinceRef.current = loadCursor(workspaceKey);
const bootstrapOk = await reloadActiveWorkspaceState(slug, workspaceKey, {
  preserveSelection: false,
});
```

Then remove the duplicated `Promise.all([...client.me, client.channels, ...])` block and the duplicated store writes.

- [x] **Step 4: Change local sync reset callback**

Replace the current `onSyncReset` callback with:

```ts
onSyncReset: () => {
  void reloadActiveWorkspaceState(slug, workspaceKey, {
    preserveSelection: true,
  }).catch(() => {
    markTransportUnavailable();
  });
},
```

Do not call `resetChatForSwitch`, `resetAgentsForSwitch`, `resetCardsForSwitch`, or `resetBoardsForSwitch` from this callback.
Do not clear the poll cursor from this callback; the next poll can still diff from the previous cursor if detail revalidation fails.

- [x] **Step 5: Handle poll reset responses**

Only save the new `commit_id` after the full reload succeeds:

```ts
if (pollRes.data.reset === true) {
  clearCursor(requestWorkspaceKey);
  sinceRef.current = undefined;
  const reloaded = await reloadActiveWorkspaceState(slug, requestWorkspaceKey, {
    preserveSelection: true,
  });
  if (reloaded) {
    sinceRef.current = nextCommitId;
    saveCursor(requestWorkspaceKey, sinceRef.current);
    setHeadCommit(sinceRef.current);
    markConnected();
  }
  return;
}
```

- [x] **Step 6: Run targeted frontend tests**

Run:

```bash
cd products/gitim/frontend
npm test -- src/hooks/use-chat-store.test.ts src/lib/client.local.test.ts
```

Expected: PASS.

## Task 2: Worker Events And Sync Results

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts`
- Modify: `products/gitim/frontend/src/lib/backend.ts`
- Modify: `products/gitim/frontend/src/lib/backend.test.ts`
- Modify: `products/gitim/frontend/src/daemon-web/sync.ts`
- Modify: `products/gitim/frontend/src/daemon-web/sync.test.ts`

- [x] **Step 1: Update Worker event types**

In `worker.ts`, change:

```ts
type: "sync_reset" | "sync_error";
```

to:

```ts
type: "sync_reset" | "repo_changed" | "sync_error" | "reconnect_required";
commit_id?: string;
reason?: "fast_forward" | "rebase" | "push";
error?: string;
error_code?: string;
needs_token?: boolean;
```

Also update `isUnscopedWorkerEvent` to recognize all four event names.

- [x] **Step 2: Update LocalBackend event handling tests**

In `backend.test.ts`, add a test that emits stale and current `repo_changed` events and expects exactly one `onSyncReset` call:

```ts
it("treats repo_changed as the scoped sync reset successor", () => {
  const onSyncReset = vi.fn();
  const backend = new LocalBackend({
    workspaceId: "ws_current",
    generation: 2,
    onSyncReset,
  });
  const worker = StubWorker.instances[0];

  worker.emit({
    type: "repo_changed",
    workspaceId: "ws_current",
    generation: 1,
    commit_id: "old",
    reason: "fast_forward",
  });
  worker.emit({
    type: "repo_changed",
    workspaceId: "ws_current",
    generation: 2,
    commit_id: "new",
    reason: "fast_forward",
  });

  expect(onSyncReset).toHaveBeenCalledTimes(1);
  backend.terminate();
});
```

- [x] **Step 3: Implement LocalBackend event handling**

In `backend.ts`, treat both `sync_reset` and `repo_changed` as reset callbacks:

```ts
if ("type" in data && (data.type === "sync_reset" || data.type === "repo_changed")) {
  this.onSyncReset?.();
  const reset = (globalThis as unknown as Record<string, unknown>).__gitimSyncReset;
  if (typeof reset === "function") reset();
  return;
}
```

For reconnect events:

```ts
if ("type" in data && data.type === "reconnect_required") {
  clearSessionToken(this.workspaceId);
  return;
}
```

- [x] **Step 4: Add `SyncResult` in `sync.ts`**

Add exported types:

```ts
export type SyncResultStatus =
  | "idle"
  | "pushed"
  | "fast_forwarded"
  | "rebased"
  | "reconnect_required";

export interface SyncResult {
  beforeHead: string;
  afterHead: string;
  changed: boolean;
  status: SyncResultStatus;
}
```

Change `syncInFlight` to `Promise<SyncResult> | null`.

- [x] **Step 5: Make `runSyncOnce` return `SyncResult`**

Use this status mapping:

- no token -> `reconnect_required`, unchanged.
- local push success -> `pushed`, changed true.
- remote equals local -> `idle`, changed false.
- remote fast-forward -> `fast_forwarded`, changed true, emit `repo_changed`.
- conflict rebase and push -> `rebased`, changed true, emit `repo_changed`.

Keep throwing for non-auth sync failures so `syncAfterCommit` still returns `commit_only`.

- [x] **Step 6: Add sync tests**

Add tests in `sync.test.ts`:

```ts
it("returns fast_forwarded and emits repo_changed for remote-only changes", async () => {
  const git = gitMocks;
  setState({ headCommit: "local-head", defaultBranch: "main" });
  git.resolveHead.mockResolvedValueOnce("local-head");
  git.resolveRemoteHead.mockResolvedValueOnce("remote-head");
  git.resolveHead.mockResolvedValueOnce("remote-head");

  const result = await runSync({ forceNewCycle: true });

  expect(result).toEqual({
    beforeHead: "local-head",
    afterHead: "remote-head",
    changed: true,
    status: "fast_forwarded",
  });
  expect(postMessageMock).toHaveBeenCalledWith({
    type: "repo_changed",
    commit_id: "remote-head",
    reason: "fast_forward",
  });
});
```

Add another test for concurrent dedupe:

```ts
it("shares an in-flight sync for concurrent non-forced calls", async () => {
  let releaseFetch!: () => void;
  gitMocks.fetchOrigin.mockImplementationOnce(
    () => new Promise<void>((resolve) => {
      releaseFetch = resolve;
    }),
  );
  setState({ headCommit: "local-head" });
  gitMocks.resolveHead.mockResolvedValue("local-head");
  gitMocks.resolveRemoteHead.mockResolvedValue("local-head");

  const first = runSync();
  const second = runSync();
  releaseFetch();

  await expect(Promise.all([first, second])).resolves.toEqual([
    {
      beforeHead: "local-head",
      afterHead: "local-head",
      changed: false,
      status: "idle",
    },
    {
      beforeHead: "local-head",
      afterHead: "local-head",
      changed: false,
      status: "idle",
    },
  ]);
  expect(gitMocks.fetchOrigin).toHaveBeenCalledTimes(1);
});
```

- [x] **Step 7: Run sync/backend tests**

Run:

```bash
cd products/gitim/frontend
npm test -- src/daemon-web/sync.test.ts src/lib/backend.test.ts
```

Expected: PASS.

## Task 3: Poll Delegates To Worker Sync

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.test.ts`
- Modify: `products/gitim/frontend/src/lib/types.ts`

- [x] **Step 1: Add poll reset typing**

In `types.ts`, add:

```ts
export interface PollResponse {
  commit_id: string;
  changes: PollChange[];
  reset?: boolean;
  sync_enabled?: boolean;
  needs_token?: boolean;
}
```

This keeps `ApiResponse<PollResponse>` available to new tests without changing the generic `ApiResponse` shape.

- [x] **Step 2: Update handler tests for sync ownership**

In `handlers.test.ts`, add:

```ts
it("poll delegates git state ownership to runSync", async () => {
  const git = vi.mocked(await import("./git"));
  git.resolveHead.mockResolvedValueOnce("remote-head");
  git.diffTrees.mockResolvedValueOnce(["channels/general.thread"]);

  const res = await poll("base");

  expect(res.ok).toBe(true);
  expect(runSyncMock).toHaveBeenCalledWith();
  expect(git.fetchOrigin).not.toHaveBeenCalled();
  expect(git.resetToRemote).not.toHaveBeenCalled();
  expect(res.data?.commit_id).toBe("remote-head");
  expect(res.data?.changes).toEqual([
    {
      channel: "general",
      kind: "new_messages",
      entries: [
        expect.objectContaining({ line_number: 1, body: "hello" }),
        expect.objectContaining({ line_number: 2, body: "reply" }),
      ],
    },
  ]);
});
```

Add stale cursor behavior:

```ts
it("returns reset on stale poll cursor instead of synthesizing full changes", async () => {
  const git = vi.mocked(await import("./git"));
  git.resolveHead.mockResolvedValueOnce("remote-head");
  git.diffTrees.mockRejectedValueOnce(new Error("bad object"));

  const res = await poll("stale-head");

  expect(res.ok).toBe(true);
  expect(res.data).toEqual({
    commit_id: "remote-head",
    changes: [],
    reset: true,
  });
});
```

- [x] **Step 3: Implement `handlers.poll` delegation**

Replace direct fetch/reset logic in `poll` with:

```ts
try {
  await runSync();
  const currentHead = await gitOps.resolveHead(s.repoDir);

  if (!since || since === currentHead) {
    return ok({ commit_id: currentHead, changes: [] });
  }

  let changedFiles: string[];
  try {
    changedFiles = await gitOps.diffTrees(s.repoDir, since, currentHead);
  } catch {
    return ok({ commit_id: currentHead, changes: [], reset: true });
  }

  // Existing change building logic remains below.
} catch (e) {
  // Existing auth handling remains.
}
```

- [x] **Step 4: Run handler tests**

Run:

```bash
cd products/gitim/frontend
npm test -- src/daemon-web/handlers.test.ts
```

Expected: PASS.

## Task 4: Full Verification And Docs Status

**Files:**
- Modify: `docs/plans/2026-05-10-mobile-worker-git-owner/design.md`
- Modify: `docs/plans/2026-05-10-mobile-worker-git-owner/plan.md`

- [x] **Step 1: Mark design status approved**

Design status now reads:

```md
Status: APPROVED (implementation complete)
```

- [x] **Step 2: Run all frontend checks**

Run:

```bash
cd products/gitim/frontend
npm test
npm run build
```

Expected:

- `npm test`: all frontend tests pass.
- `npm run build`: `tsc -b && vite build` succeeds.

- [x] **Step 3: Run diff checks**

Run:

```bash
git diff --check
git status --short
```

Expected: no whitespace errors. Status should only show the intended files.

- [x] **Step 4: Commit implementation**

Run:

```bash
git add products/gitim/frontend/src/app.tsx \
  products/gitim/frontend/src/lib/types.ts \
  products/gitim/frontend/src/lib/backend.ts \
  products/gitim/frontend/src/lib/backend.test.ts \
  products/gitim/frontend/src/daemon-web/worker.ts \
  products/gitim/frontend/src/daemon-web/sync.ts \
  products/gitim/frontend/src/daemon-web/sync.test.ts \
  products/gitim/frontend/src/daemon-web/handlers.ts \
  products/gitim/frontend/src/daemon-web/handlers.test.ts \
  docs/plans/2026-05-10-mobile-worker-git-owner/design.md \
  docs/plans/2026-05-10-mobile-worker-git-owner/plan.md
git commit -m "fix(frontend): serialize browser workspace sync"
```
