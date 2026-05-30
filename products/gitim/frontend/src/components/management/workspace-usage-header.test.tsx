// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { Agent, UsageBucket, UsageDayEntry, UsageSummary } from "@/lib/types";

const testEnv = vi.hoisted(() => {
  function createMemoryStorage(): Storage {
    const values = new Map<string, string>();
    return {
      get length() {
        return values.size;
      },
      clear() {
        values.clear();
      },
      getItem(key: string) {
        return values.get(key) ?? null;
      },
      key(index: number) {
        return Array.from(values.keys())[index] ?? null;
      },
      removeItem(key: string) {
        values.delete(key);
      },
      setItem(key: string, value: string) {
        values.set(key, value);
      },
    };
  }

  const localStorage = createMemoryStorage();
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: localStorage,
  });
  return { localStorage };
});

import { WorkspaceUsageHeader } from "./workspace-usage-header";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function bucket(input: number, output = 0, turns = 0): UsageBucket {
  return { input, output, cacheRead: 0, cacheCreation: 0, turns };
}

function summary(
  totals: UsageBucket,
  today: UsageBucket,
  byDay: UsageDayEntry[] = [],
): UsageSummary {
  return {
    providerReportsUsage: true,
    firstSeen: "2026-05-01T00:00:00Z",
    lastUpdated: "2026-05-18T12:00:00Z",
    totals,
    today,
    byDay,
  };
}

function agent(id: string, provider: string, usageSummary: UsageSummary): Agent {
  return {
    id,
    handler: id,
    name: id,
    status: "running",
    provider: provider as Agent["provider"],
    systemPrompt: "",
    repoPath: `/tmp/${id}`,
    messagesProcessed: 0,
    usageSummary,
  };
}

function bareAgent(id: string): Agent {
  return {
    id,
    handler: id,
    name: id,
    status: "running",
    systemPrompt: "",
    repoPath: `/tmp/${id}`,
    messagesProcessed: 0,
  };
}

describe("WorkspaceUsageHeader", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    testEnv.localStorage.clear();
  });

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    container.remove();
    testEnv.localStorage.clear();
  });

  it("puts recent usage first and cumulative usage after it", () => {
    act(() => {
      root?.render(
        <WorkspaceUsageHeader
          label="Fleet Usage"
          agents={[
            agent(
              "alice",
              "codex",
              summary(bucket(300, 0, 9), bucket(30, 0, 1), [
                { date: "2026-05-17", bucket: bucket(270, 0, 8) },
                { date: "2026-05-18", bucket: bucket(30, 0, 1) },
              ]),
            ),
            agent("bob", "claude", summary(bucket(20, 0, 1), bucket(10, 0, 1))),
          ]}
        />,
      );
    });

    const text = container.textContent ?? "";
    const todayRow = container.querySelector('[data-testid="workspace-usage-today"]');
    const totalRow = container.querySelector('[data-testid="workspace-usage-total"]');
    expect(text).toContain("近日");
    expect(text).toContain("今日 40");
    expect(text).toContain("累计 320");
    expect(todayRow?.textContent ?? "").toContain("codex");
    expect(todayRow?.textContent ?? "").toContain("30");
    expect(totalRow?.textContent ?? "").toContain("codex");
    expect(totalRow?.textContent ?? "").toContain("300");
    expect(text.indexOf("近日")).toBeLessThan(text.indexOf("累计"));
  });

  it("switches both today and cumulative detail rows to handler grouping", () => {
    act(() => {
      root?.render(
        <WorkspaceUsageHeader
          agents={[
            agent("alice", "codex", summary(bucket(300, 0, 9), bucket(30, 0, 1))),
            agent("bob", "codex", summary(bucket(100, 0, 4), bucket(40, 0, 2))),
          ]}
        />,
      );
    });

    const handlerButton = Array.from(container.querySelectorAll("button")).find(
      (button) => button.textContent === "Handler",
    );
    act(() => {
      handlerButton?.click();
    });

    const todayRow = container.querySelector('[data-testid="workspace-usage-today"]');
    const totalRow = container.querySelector('[data-testid="workspace-usage-total"]');
    expect(todayRow?.textContent ?? "").toContain("alice");
    expect(todayRow?.textContent ?? "").toContain("30");
    expect(todayRow?.textContent ?? "").toContain("bob");
    expect(todayRow?.textContent ?? "").toContain("40");
    expect(totalRow?.textContent ?? "").toContain("alice");
    expect(totalRow?.textContent ?? "").toContain("300");
    expect(totalRow?.textContent ?? "").toContain("bob");
    expect(totalRow?.textContent ?? "").toContain("100");
  });

  it("renders workload even before token usage exists", () => {
    act(() => {
      root?.render(
        <WorkspaceUsageHeader
          agents={[bareAgent("alice"), bareAgent("bob"), bareAgent("cara")]}
          workload={{ working: 2, total: 3 }}
        />,
      );
    });

    const workload = container.querySelector('[data-testid="workspace-workload"]');
    expect(workload?.textContent ?? "").toContain("Working 2/3");
    expect(container.querySelector('[data-testid="workspace-usage-today"]')).toBeNull();
  });
});
