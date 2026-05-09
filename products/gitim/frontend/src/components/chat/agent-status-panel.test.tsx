// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

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

import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import type { Agent } from "@/lib/types";
import { AgentStatusPanel } from "./agent-status-panel";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function agentWithUsage(usedPercent: number): Agent {
  return {
    id: "cfo",
    name: "cfo",
    status: "running",
    systemPrompt: "",
    repoPath: "/tmp/cfo",
    messagesProcessed: 0,
    sessionUsage: {
      sessionId: "019e0bd5",
      inputTokens: 50_000,
      outputTokens: 500,
      maxTokens: 100_000,
      usedPercent,
      source: "provider_reported",
      updatedAt: "2026-05-09T10:00:00Z",
    },
  };
}

describe("AgentStatusPanel", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    useAgentStore.setState({ agents: [], selectedAgentId: null });
    useAgentActivityStore.setState({ activities: {}, lastSlug: null });
  });

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
  });

  it("renders session usage as a left-to-right liquid fill", async () => {
    useAgentStore.setState({ agents: [agentWithUsage(47.5)], selectedAgentId: null });

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<AgentStatusPanel />);
      await Promise.resolve();
    });

    const fill = container.querySelector<HTMLElement>(
      '[data-testid="agent-usage-liquid"]',
    );

    expect(fill).not.toBeNull();
    expect(fill?.style.getPropertyValue("--agent-usage-fill")).toBe("47.5%");
    expect(fill?.getAttribute("aria-hidden")).toBe("true");
  });
});
