// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppShell } from "./app-shell";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

vi.mock("../../hooks/use-chat-store", () => ({
  useChatStore: (
    selector: (state: { currentUser: string | null }) => unknown,
  ) => selector({ currentUser: null }),
}));

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

vi.mock("../theme/theme-toggle", () => ({
  ThemeToggle: () => null,
}));

vi.mock("../mobile/mobile-tab-bar", () => ({
  MobileTabBar: () => null,
}));

vi.mock("../sessions/quick-session-hub", () => ({
  QuickSessionHub: () => null,
}));

vi.mock("./nav-tabs", () => ({
  NavTabs: () => null,
}));

vi.mock("./connection-status-button", () => ({
  ConnectionStatusButton: () => null,
}));

describe("AppShell", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
  });

  it("does not expose donation controls in the header", async () => {
    await act(async () => {
      root.render(
        <MemoryRouter>
          <AppShell>content</AppShell>
        </MemoryRouter>,
      );
    });

    expect(
      container.querySelector("[title='Support developer']"),
    ).toBeNull();
  });
});
