// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import type { Agent } from "@/lib/types";
import { AgentCard } from "./agent-card";
import { agentModelLabel } from "./agent-model-label";

function agent(provider: Agent["provider"], model?: string): Agent {
  return {
    id: `${provider}-agent`,
    name: `${provider}-agent`,
    status: "running",
    provider,
    model,
    systemPrompt: "",
    repoPath: `/tmp/${provider}-agent`,
    messagesProcessed: 0,
  };
}

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

describe("agentModelLabel", () => {
  it("renders Kimi default model mode instead of an empty dash", () => {
    expect(agentModelLabel(agent("kimi"))).toBe("default");
  });

  it("renders explicit Kimi models verbatim", () => {
    expect(agentModelLabel(agent("kimi", "kimi-code/kimi-for-coding"))).toBe(
      "kimi-code/kimi-for-coding",
    );
  });
});

describe("AgentCard compact layout", () => {
  let root: Root | null = null;

  afterEach(() => {
    act(() => {
      root?.unmount();
    });
    root = null;
    document.body.innerHTML = "";
  });

  it("shows a bounded introduction preview with full text on hover", () => {
    const longIntroduction =
      "This is a deliberately long operating note that should appear near the status area without making the control plane row taller.";
    const record: Agent = {
      ...agent("codex", "gpt-5.5"),
      introduction: longIntroduction,
      repoPath: "/tmp/gitim/codex-agent",
      lastActivity: "2026-05-19T08:00:00Z",
      messagesProcessed: 7,
    };
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    act(() => {
      root?.render(
        <MemoryRouter>
          <AgentCard agent={record} />
        </MemoryRouter>,
      );
    });

    const summary = container.querySelector<HTMLElement>(
      '[data-testid="agent-card-summary"]',
    );
    const intro = container.querySelector<HTMLElement>(
      '[data-testid="agent-card-introduction"]',
    );
    expect(summary).not.toBeNull();
    expect(intro).not.toBeNull();
    expect(intro?.getAttribute("title")).toBe(longIntroduction);
    expect(intro?.textContent).toContain("This is a deliberately long");
    expect(intro?.textContent).toContain("...");
    expect(intro?.textContent).not.toBe(longIntroduction);
  });
});
