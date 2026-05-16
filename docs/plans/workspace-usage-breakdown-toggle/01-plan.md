# Workspace usage breakdown toggle — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a Provider/Handler segmented toggle to `WorkspaceUsageHeader` that switches the breakdown row's grouping dimension and persists the choice per-workspace via `lib/ui-state.ts`.

**Architecture:** Three thin layers, all client-side. (1) `aggregateWorkspaceUsage` gains a `byHandler` field and `byProvider` is reshaped to enumerate every distinct provider including zero-usage ones. (2) `UiState` gains `usageBreakdown: "provider" | "handler"` with narrowed validation. (3) `WorkspaceUsageHeader` renders one breakdown JSX driven by a `useState` initialised from `readUiState`; click handlers write back via `writeUiState`. No backend changes — `AgentInfo.usage_summary` already carries the data.

**Tech Stack:** React 19, TypeScript, vitest, Radix UI primitives, tailwind. Existing project conventions only — no new deps.

**Scope-out (explicit, do NOT add):**
- Cross-instance real-time sync within the same tab (fleet-mode `${title} Usage` headers will only reconcile on next render trigger, not on every keystroke).
- Cross-tab sync. localStorage `storage` event hookup is v2 polish.
- Backend changes / new endpoints.

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `products/gitim/frontend/src/hooks/use-workspace-usage.ts` | Modify | Aggregator gains `byHandler`; `byProvider` reshaped to include zero-usage providers |
| `products/gitim/frontend/src/hooks/use-workspace-usage.test.ts` | Modify | New cases for `byHandler` + zero-handler + zero-provider; tighten existing sort tests |
| `products/gitim/frontend/src/lib/ui-state.ts` | Modify | `UsageBreakdown` type, `UiState.usageBreakdown`, default, narrow parser |
| `products/gitim/frontend/src/lib/ui-state.test.ts` | Modify | Default, valid round-trip, legacy localStorage without field, invalid value fallback |
| `products/gitim/frontend/src/components/management/workspace-usage-header.tsx` | Modify | Read/write `usageBreakdown`; render toggle; single render path picks `byProvider` or `byHandler` |

---

## Task 1: Aggregator — add `byHandler`, reshape `byProvider` to allow zeros

**Files:**
- Modify: `products/gitim/frontend/src/hooks/use-workspace-usage.ts`
- Test: `products/gitim/frontend/src/hooks/use-workspace-usage.test.ts`

### Step 1.1: Adjust existing tests for new sort tiebreaker

The existing `groups totals by provider and sorts descending` test will still pass because tokens differ. But add a same-total tiebreaker case so the new `localeCompare` rule is locked in.

- [ ] **Write the new sort-tiebreaker test**

Append to the existing `describe("aggregateWorkspaceUsage", ...)` in [use-workspace-usage.test.ts](products/gitim/frontend/src/hooks/use-workspace-usage.test.ts):

```ts
it("sorts byProvider by token total desc, alphabetical on ties", () => {
  const out = aggregateWorkspaceUsage([
    agent("a", "codex", summary(bucket(100, 0), bucket(0, 0))),
    agent("b", "claude", summary(bucket(100, 0), bucket(0, 0))),
    agent("c", "opencode", summary(bucket(50, 0), bucket(0, 0))),
  ]);
  expect(out.byProvider.map((p) => p.provider)).toEqual([
    "claude",
    "codex",
    "opencode",
  ]);
});
```

- [ ] **Run it to verify it FAILS**

`cd products/gitim/frontend && npx vitest run src/hooks/use-workspace-usage.test.ts -t "tiebreaker"`

Expected: FAIL — current sort is by token desc only; with equal tokens the order depends on Map insertion order (codex first, then claude).

### Step 1.2: Add `byHandler` shape + zero-enumeration test

- [ ] **Extend the test file with three new cases**

Append:

```ts
it("byHandler enumerates every agent including zero-usage ones", () => {
  const out = aggregateWorkspaceUsage([
    agent("alice", "codex", summary(bucket(100, 50), bucket(40, 20))),
    agent("bob", "claude"),  // no usageSummary
    agent("carol", "pi", summary(bucket(10, 5), bucket(0, 0))),
  ]);
  expect(out.byHandler.map((h) => h.handler)).toEqual([
    "alice",  // 150 tokens
    "carol",  //  15 tokens
    "bob",    //   0, sorted last
  ]);
  expect(out.byHandler.find((h) => h.handler === "bob")?.bucket).toEqual({
    input: 0,
    output: 0,
    cacheRead: 0,
    cacheCreation: 0,
    turns: 0,
  });
});

it("byProvider enumerates every distinct provider including zero-usage ones", () => {
  const out = aggregateWorkspaceUsage([
    agent("alice", "codex", summary(bucket(100, 50), bucket(40, 20))),
    agent("bob", "claude"),  // contributes 0 to claude
  ]);
  expect(out.byProvider.map((p) => p.provider)).toEqual(["codex", "claude"]);
  expect(out.byProvider.find((p) => p.provider === "claude")?.bucket.input).toBe(0);
});

it("hasData=false still hides byHandler entries (whole header hidden)", () => {
  // We don't need byHandler to populate when no agent has usage — the header
  // hides entirely. Confirm hasData=false short-circuits.
  const out = aggregateWorkspaceUsage([
    agent("a", "claude"),
    agent("b", "codex"),
  ]);
  expect(out.hasData).toBe(false);
  expect(out.byHandler).toEqual([]);
});
```

- [ ] **Run all three; expect FAIL**

`cd products/gitim/frontend && npx vitest run src/hooks/use-workspace-usage.test.ts -t "byHandler|byProvider enumerates|byHandler entries"`

Expected: 3 FAIL — `byHandler` field doesn't exist yet; the existing `byProvider` filters out agents without `usageSummary`.

### Step 1.3: Update the aggregator

- [ ] **Modify `use-workspace-usage.ts`**

Replace the `WorkspaceUsage` interface to add `byHandler`:

```ts
export interface WorkspaceUsage {
  totals: UsageBucket;
  today: UsageBucket;
  byDay: UsageDayEntry[];
  byProvider: { provider: string; bucket: UsageBucket }[];
  byHandler: { handler: string; bucket: UsageBucket }[];
  hasData: boolean;
}
```

Update `EMPTY_USAGE`:

```ts
const EMPTY_USAGE: WorkspaceUsage = {
  totals: ZERO_BUCKET,
  today: ZERO_BUCKET,
  byDay: [],
  byProvider: [],
  byHandler: [],
  hasData: false,
};
```

Rewrite `aggregateWorkspaceUsage` — keep the `summaries` filter for totals/today/byDay (those legitimately ignore agents without data), but build `byProvider` and `byHandler` over the **full** agent list so zero entries appear:

```ts
export function aggregateWorkspaceUsage(agents: Agent[]): WorkspaceUsage {
  const summaries = agents
    .map((a) => ({ provider: a.provider ?? "unknown", summary: a.usageSummary }))
    .filter(
      (e): e is { provider: string; summary: UsageSummary } =>
        e.summary !== undefined,
    );
  if (summaries.length === 0) return EMPTY_USAGE;

  const totals = mergeBuckets(summaries.map((e) => e.summary.totals));
  const today = mergeBuckets(summaries.map((e) => e.summary.today));
  const byDay = mergeByDay(summaries.map((e) => e.summary.byDay));

  // byProvider: enumerate every distinct provider across ALL agents, including
  // those without a summary (they contribute a ZERO_BUCKET). Sort token-desc
  // with alphabetical tiebreaker for stable ordering when values match.
  const providerMap = new Map<string, UsageBucket[]>();
  for (const a of agents) {
    const key = a.provider ?? "unknown";
    const arr = providerMap.get(key) ?? [];
    arr.push(a.usageSummary?.totals ?? { ...ZERO_BUCKET });
    providerMap.set(key, arr);
  }
  const byProvider = Array.from(providerMap.entries())
    .map(([provider, buckets]) => ({ provider, bucket: mergeBuckets(buckets) }))
    .sort(compareEntry((e) => e.provider));

  // byHandler: one entry per agent (handler is unique per agent), zero-usage
  // agents render as 0.
  const byHandler = agents
    .map((a) => ({
      handler: a.id,
      bucket: a.usageSummary?.totals ?? { ...ZERO_BUCKET },
    }))
    .sort(compareEntry((e) => e.handler));

  return { totals, today, byDay, byProvider, byHandler, hasData: true };
}

function compareEntry<T extends { bucket: UsageBucket }>(
  labelOf: (e: T) => string,
) {
  return (a: T, b: T) => {
    const diff = totalSum(b.bucket) - totalSum(a.bucket);
    return diff !== 0 ? diff : labelOf(a).localeCompare(labelOf(b));
  };
}
```

Note: `Agent.id` is the handler in this codebase — see [client.ts:1314](products/gitim/frontend/src/lib/client.ts) where `id: (raw.id ?? raw.handler)` does the mapping. We use `a.id` here and label the output field `handler` because that's what the user sees and what `byProvider` symmetry demands.

- [ ] **Run all aggregator tests; expect PASS**

`cd products/gitim/frontend && npx vitest run src/hooks/use-workspace-usage.test.ts`

Expected: all PASS (existing 5 cases + 4 new = 9 total).

### Step 1.4: Commit

- [ ] **Commit**

```bash
git add products/gitim/frontend/src/hooks/use-workspace-usage.ts \
        products/gitim/frontend/src/hooks/use-workspace-usage.test.ts
git commit
```

Use commit message:

```
feat(usage): aggregator gains byHandler + zero-aware byProvider

- byHandler: one entry per agent, zero-usage agents render as 0
- byProvider: enumerate distinct providers including zero contributors
- shared sort rule: token-desc with alphabetical tiebreaker
```

---

## Task 2: `ui-state.ts` — `usageBreakdown` field

**Files:**
- Modify: `products/gitim/frontend/src/lib/ui-state.ts`
- Test: `products/gitim/frontend/src/lib/ui-state.test.ts`

### Step 2.1: Write tests for the new field

- [ ] **Append to `ui-state.test.ts` describe block**

```ts
it("defaults usageBreakdown to 'provider'", () => {
  expect(readUiState("runtime:myws").usageBreakdown).toBe("provider");
});

it("round-trips a valid usageBreakdown value", () => {
  writeUiState("runtime:myws", { usageBreakdown: "handler" });
  expect(readUiState("runtime:myws").usageBreakdown).toBe("handler");
});

it("falls back to default when persisted usageBreakdown is invalid", () => {
  localStorage.setItem(
    "gitim-ui-state:runtime:myws",
    JSON.stringify({ usageBreakdown: "bogus" }),
  );
  expect(readUiState("runtime:myws").usageBreakdown).toBe("provider");
});

it("falls back to default when persisted state lacks usageBreakdown", () => {
  // Simulates legacy localStorage from before this field was introduced.
  localStorage.setItem(
    "gitim-ui-state:runtime:myws",
    JSON.stringify({ channel: "general", cardsShowArchived: true }),
  );
  const state = readUiState("runtime:myws");
  expect(state.usageBreakdown).toBe("provider");
  expect(state.channel).toBe("general");
  expect(state.cardsShowArchived).toBe(true);
});
```

- [ ] **Run; expect FAIL**

`cd products/gitim/frontend && npx vitest run src/lib/ui-state.test.ts -t "usageBreakdown"`

Expected: 4 FAIL — field/type don't exist yet.

### Step 2.2: Implement

- [ ] **Modify `ui-state.ts`**

Add the type and extend `UiState` + `DEFAULT_UI_STATE`:

```ts
export type UsageBreakdown = "provider" | "handler";

export interface UiState {
  channel: string | null;
  boardHandler: string | null;
  cardsShowArchived: boolean;
  usageBreakdown: UsageBreakdown;
}

export const DEFAULT_UI_STATE: UiState = {
  channel: null,
  boardHandler: null,
  cardsShowArchived: false,
  usageBreakdown: "provider",
};
```

Inside `readUiState`, extend the return object with narrow validation:

```ts
return {
  channel: typeof obj.channel === "string" ? obj.channel : DEFAULT_UI_STATE.channel,
  boardHandler:
    typeof obj.boardHandler === "string" ? obj.boardHandler : DEFAULT_UI_STATE.boardHandler,
  cardsShowArchived:
    typeof obj.cardsShowArchived === "boolean"
      ? obj.cardsShowArchived
      : DEFAULT_UI_STATE.cardsShowArchived,
  usageBreakdown:
    obj.usageBreakdown === "provider" || obj.usageBreakdown === "handler"
      ? obj.usageBreakdown
      : DEFAULT_UI_STATE.usageBreakdown,
};
```

- [ ] **Run; expect PASS**

`cd products/gitim/frontend && npx vitest run src/lib/ui-state.test.ts`

Expected: all PASS (existing cases + 4 new).

### Step 2.3: Commit

- [ ] **Commit**

```bash
git add products/gitim/frontend/src/lib/ui-state.ts \
        products/gitim/frontend/src/lib/ui-state.test.ts
git commit
```

Message:

```
feat(ui-state): persist usageBreakdown toggle per workspace

- "provider" | "handler", defaults to "provider"
- narrow parser rejects bogus values, legacy localStorage upgrades cleanly
```

---

## Task 3: `WorkspaceUsageHeader` — toggle control + unified render path

**Files:**
- Modify: `products/gitim/frontend/src/components/management/workspace-usage-header.tsx`

### Step 3.1: Inspect surrounding imports

- [ ] **Read the current file**

Re-read [workspace-usage-header.tsx](products/gitim/frontend/src/components/management/workspace-usage-header.tsx) so the rewrite stays in style with existing imports.

### Step 3.2: Replace the component implementation

- [ ] **Rewrite the file**

Replace the entire file body (keep the file path, replace the contents):

```tsx
import { useEffect, useMemo, useState } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { workspaceIdentity } from "@/lib/workspace-key";
import { formatTokens } from "@/lib/format-tokens";
import { sparklinePath } from "@/lib/sparkline";
import {
  DEFAULT_UI_STATE,
  readUiState,
  writeUiState,
  type UsageBreakdown,
} from "@/lib/ui-state";
import { aggregateWorkspaceUsage, useWorkspaceUsage } from "@/hooks/use-workspace-usage";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { Agent, UsageBucket } from "@/lib/types";

function bucketTotal(b: UsageBucket): number {
  return b.input + b.output + b.cacheRead + b.cacheCreation;
}

interface WorkspaceUsageHeaderProps {
  agents?: Agent[];
  label?: string;
  className?: string;
}

/** Header strip rendered above the agents grid on the management page.
 *  Sums every agent's `usageSummary` client-side and renders the workspace-
 *  level totals + 30-day sparkline + breakdown. The breakdown grouping
 *  dimension (Provider | Handler) is user-controlled and persists per
 *  workspace via `lib/ui-state.ts`. Hides itself when no agent has
 *  produced usage data yet. */
export function WorkspaceUsageHeader({
  agents,
  label = "Workspace Usage",
  className = "mb-4",
}: WorkspaceUsageHeaderProps) {
  const storeUsage = useWorkspaceUsage();
  const propUsage = useMemo(
    () => (agents ? aggregateWorkspaceUsage(agents) : null),
    [agents],
  );
  const usage = propUsage ?? storeUsage;

  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeWorkspace = activeSlug
    ? workspaces.find((w) => w.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;

  const [breakdown, setBreakdown] = useState<UsageBreakdown>(() =>
    workspaceKey
      ? readUiState(workspaceKey).usageBreakdown
      : DEFAULT_UI_STATE.usageBreakdown,
  );

  // Re-hydrate when workspace key changes (e.g. user switches workspaces
  // without remounting this component).
  useEffect(() => {
    if (!workspaceKey) return;
    setBreakdown(readUiState(workspaceKey).usageBreakdown);
  }, [workspaceKey]);

  if (!usage.hasData) return null;

  const totalTokens = bucketTotal(usage.totals);
  const todayTokens = bucketTotal(usage.today);
  const sparklineValues = usage.byDay.map((d) => bucketTotal(d.bucket));

  const entries =
    breakdown === "provider"
      ? usage.byProvider.map((e) => ({ key: e.provider, label: e.provider, bucket: e.bucket }))
      : usage.byHandler.map((e) => ({ key: e.handler, label: e.handler, bucket: e.bucket }));

  function selectBreakdown(next: UsageBreakdown) {
    setBreakdown(next);
    if (workspaceKey) writeUiState(workspaceKey, { usageBreakdown: next });
  }

  return (
    <section
      className={`${className} rounded-lg border border-border-soft bg-card/40 px-4 py-3 flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between`}
    >
      <div className="flex flex-col gap-1 min-w-0">
        <div className="text-xs uppercase tracking-wide text-text-muted">
          {label}
        </div>
        <div className="flex items-baseline gap-2">
          <span className="text-xl font-mono text-foreground">
            {formatTokens(totalTokens)}
          </span>
          <span className="text-sm text-text-secondary">
            累计 · 今日 {formatTokens(todayTokens)} · 今日 {usage.today.turns} turns
          </span>
        </div>
        <div className="flex flex-wrap items-center gap-x-3 gap-y-1 text-xs font-mono text-text-muted">
          <div
            role="group"
            aria-label="Usage breakdown grouping"
            className="flex shrink-0 items-center gap-1"
          >
            <BreakdownButton
              active={breakdown === "provider"}
              onClick={() => selectBreakdown("provider")}
            >
              Provider
            </BreakdownButton>
            <BreakdownButton
              active={breakdown === "handler"}
              onClick={() => selectBreakdown("handler")}
            >
              Handler
            </BreakdownButton>
          </div>
          {entries.map(({ key, label: l, bucket }) => (
            <span key={key}>
              {l} {formatTokens(bucketTotal(bucket))}
            </span>
          ))}
        </div>
      </div>
      {sparklineValues.length > 0 && (
        <div className="text-primary shrink-0">
          <svg
            width={180}
            height={36}
            viewBox="0 0 180 36"
            aria-label="近 30 天 workspace token 用量"
            className="overflow-visible"
          >
            <path
              d={sparklinePath(sparklineValues, 180, 36)}
              fill="none"
              stroke="currentColor"
              strokeWidth={1.5}
              strokeLinecap="round"
              strokeLinejoin="round"
            />
          </svg>
        </div>
      )}
    </section>
  );
}

function BreakdownButton({
  active,
  onClick,
  children,
}: {
  active: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <Button
      type="button"
      size="sm"
      variant={active ? "default" : "ghost"}
      onClick={onClick}
      aria-pressed={active}
      className={cn(
        "h-6 px-2 text-[10px] uppercase tracking-wide",
        active
          ? "bg-accent-muted text-primary hover:bg-accent-muted hover:text-primary"
          : "text-muted-foreground",
      )}
    >
      {children}
    </Button>
  );
}
```

Why this exact shape:
- The toggle lives inside the breakdown `<div>` so it shares the `flex-wrap` line with the token entries; `shrink-0` on the toggle group keeps it left-anchored when the agent count overflows.
- `BreakdownButton` is a tiny local helper to avoid duplicating the variant/className tangle twice.
- `useEffect` re-hydrates `breakdown` when `workspaceKey` changes; without this, switching workspaces would leave the toggle stale.
- The button styling mirrors `card-filter-bar.tsx::handleToggleArchived` (`variant="default"` when active, accent-muted background, `text-muted-foreground` when inactive) so the visual language matches the rest of the app.

### Step 3.3: Typecheck

- [ ] **Run typecheck**

`cd products/gitim/frontend && npx tsc --noEmit`

Expected: no errors.

If any import path differs from this codebase, fix at runtime — [card-filter-bar.tsx](products/gitim/frontend/src/components/cards/card-filter-bar.tsx) is the canonical reference for `workspaceIdentity` / `useConnectionStore` / `useWorkspaceStore` import paths.

### Step 3.4: Targeted vitest run for callers

- [ ] **Run agent-list test (the only test that exercises this component path)**

`cd products/gitim/frontend && npx vitest run src/components/management/agent-list.test.tsx`

Expected: PASS (no regression — the toggle has a default that matches old behaviour).

### Step 3.5: Commit

- [ ] **Commit**

```bash
git add products/gitim/frontend/src/components/management/workspace-usage-header.tsx
git commit
```

Message:

```
feat(usage): add Provider/Handler breakdown toggle to workspace header

- Two-button segmented control inline with the breakdown row
- Selection persists per workspace via lib/ui-state.ts
- Re-hydrates on workspace switch via useEffect
- Single render path; switching dimensions just swaps the data source
```

---

## Task 4: Final sweep — full frontend test + lint

**Files:** none modified; verification only.

- [ ] **Run the full frontend test suite**

`cd products/gitim/frontend && npx vitest run`

Expected: all PASS. If anything outside this feature's surface fails, investigate before declaring done.

- [ ] **Run typecheck across the frontend**

`cd products/gitim/frontend && npx tsc --noEmit`

Expected: clean.

- [ ] **Run lint if the project has one**

`cd products/gitim/frontend && npx eslint src --ext .ts,.tsx --max-warnings 0`

Expected: clean. If lint config not present or errors only in unrelated files, note it in the PR description rather than fixing.

- [ ] **Manual smoke (visual)**

Skip if no dev server reachable; otherwise:

```bash
cd products/gitim/frontend && npm run dev
```

In the browser: open management page → confirm two-button toggle appears in the WORKSPACE USAGE row → click Handler → see per-handler entries → reload → toggle still on Handler → switch workspace → toggle resets according to that workspace's stored value.

- [ ] **No commit needed (verification only)**

If any failures surfaced fixes, those would be their own commits inside the relevant task.

---

## Self-Review (run before declaring the plan done)

1. **Spec coverage** — Every acceptance criterion in [00-requirements.md](docs/plans/workspace-usage-breakdown-toggle/00-requirements.md):
   - Toggle control ✅ Task 3
   - Switch updates breakdown row only ✅ Task 3 (entries derive from `breakdown`)
   - Persists across reload ✅ Task 3 + Task 2 (write/read)
   - Per-workspace not global ✅ Task 3 (`workspaceKey` key)
   - Handler dimension lists all agents w/ 0 ✅ Task 1 (`byHandler` over full agent list)
   - Provider dimension lists distinct providers w/ 0 ✅ Task 1 (`providerMap` over full agent list)
   - Aggregator unit test ✅ Task 1
   - ui-state validation test ✅ Task 2
   - No a11y regression ✅ Task 3 (`aria-pressed`, `role="group"`)

2. **Placeholder scan** — searched plan body for `TBD|TODO|fill in|implement later|similar to|appropriate error|edge cases`. None found.

3. **Type consistency** — `UsageBreakdown` defined Task 2, consumed Task 3 with the same import. `byHandler` shape `{ handler, bucket }` defined Task 1, consumed Task 3 with the same destructuring. `compareEntry` is a private helper, not re-exported.
