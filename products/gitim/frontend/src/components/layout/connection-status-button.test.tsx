// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { ConnectionStatusButton } from "./connection-status-button";
import { useChatStore } from "@/hooks/use-chat-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useConnectionDiagnosticsStore } from "@/hooks/use-connection-diagnostics-store";

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

vi.mock("../workspace/workspace-switcher", () => ({
  WorkspaceSwitcher: () => null,
}));

vi.mock("../update-indicator", () => ({
  UpdateIndicator: () => null,
}));

vi.mock("../usage-indicator", () => ({
  UsageIndicator: () => null,
}));

vi.mock("../timezone-toggle", () => ({
  TimezoneToggle: () => null,
}));

vi.mock("../donate-dialog", () => ({
  DonateDialog: () => null,
}));

describe("ConnectionStatusButton", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    testEnv.localStorage.clear();
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    useChatStore.getState().setConnected(false);
    useConnectionStore.setState({
      mode: "local",
      status: "ready",
      port: null,
      runtimeVersion: null,
      headCommit: "abc123456789",
      error: null,
      localReady: true,
    });
    useConnectionDiagnosticsStore.getState().reset();
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
    document.body.innerHTML = "";
  });

  it("opens browser sync diagnostics from the header status dot", async () => {
    useConnectionDiagnosticsStore.getState().recordBrowserSyncEvent({
      status: "error",
      error: "Failed to fetch via CORS proxy",
      corsProxy: "https://cors.isomorphic-git.org",
      remoteUrl: "https://github.com/acme/team.git",
    });

    await act(async () => {
      root.render(
        <MemoryRouter>
          <ConnectionStatusButton />
        </MemoryRouter>,
      );
    });

    const button = container.querySelector<HTMLButtonElement>(
      "button[aria-label='Connection diagnostics']",
    );
    expect(button).not.toBeNull();

    await act(async () => {
      button?.click();
    });

    expect(document.body.textContent).toContain("Connection diagnostics");
    expect(document.body.textContent).toContain("Browser sync");
    expect(document.body.textContent).toContain("Failed to fetch via CORS proxy");
    expect(document.body.textContent).toContain("https://cors.isomorphic-git.org");
  });
});
