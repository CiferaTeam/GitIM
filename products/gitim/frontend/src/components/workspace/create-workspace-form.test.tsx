// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { CreateWorkspaceForm } from "./create-workspace-form";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";

vi.mock("@/lib/client", async () => {
  const actual = await vi.importActual<typeof import("@/lib/client")>(
    "@/lib/client",
  );
  return {
    ...actual,
    pickWorkspaceDirectory: vi.fn(),
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

describe("CreateWorkspaceForm workspace folder picker", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    useWorkspaceStore.setState({
      create: vi.fn(),
      clearError: vi.fn(),
      error: null,
      errorCode: null,
    });
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("opens the native folder picker and derives the slug from the selected folder", async () => {
    vi.mocked(client.pickWorkspaceDirectory).mockResolvedValue({
      ok: true,
      data: { path: "/Users/dev/Workspaces/team-alpha" },
    });

    act(() => {
      root.render(<CreateWorkspaceForm />);
    });

    const chooseButton = container.querySelector<HTMLButtonElement>(
      "[data-testid='ws-folder-picker']",
    );
    expect(chooseButton).not.toBeNull();

    await act(async () => {
      chooseButton?.click();
      await Promise.resolve();
    });

    expect(client.pickWorkspaceDirectory).toHaveBeenCalledOnce();
    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-path']"),
    ).toMatchObject({
      value: "/Users/dev/Workspaces/team-alpha",
      readOnly: true,
    });
    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-slug']"),
    ).toHaveProperty("value", "team-alpha");
  });

  it("keeps the previous folder when the native picker is cancelled", async () => {
    vi.mocked(client.pickWorkspaceDirectory).mockResolvedValue({
      ok: true,
      data: { path: null },
    });

    act(() => {
      root.render(
        <CreateWorkspaceForm
          initial={{ path: "/Users/dev/Workspaces/existing" }}
        />,
      );
    });

    const chooseButton = container.querySelector<HTMLButtonElement>(
      "[data-testid='ws-folder-picker']",
    );
    expect(chooseButton).not.toBeNull();

    await act(async () => {
      chooseButton?.click();
      await Promise.resolve();
    });

    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-path']"),
    ).toHaveProperty("value", "/Users/dev/Workspaces/existing");
    expect(container.querySelector("[data-testid='ws-create-error']")).toBeNull();
  });

  it("surfaces native picker failures without clearing the selected folder", async () => {
    vi.mocked(client.pickWorkspaceDirectory).mockResolvedValue({
      ok: false,
      error: "Could not open the macOS folder picker.",
      error_code: "directory_picker_failed",
    });

    act(() => {
      root.render(
        <CreateWorkspaceForm
          initial={{ path: "/Users/dev/Workspaces/existing" }}
        />,
      );
    });

    const chooseButton = container.querySelector<HTMLButtonElement>(
      "[data-testid='ws-folder-picker']",
    );
    expect(chooseButton).not.toBeNull();

    await act(async () => {
      chooseButton?.click();
      await Promise.resolve();
    });

    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-path']"),
    ).toHaveProperty("value", "/Users/dev/Workspaces/existing");
    expect(
      container.querySelector("[data-testid='ws-create-error']")?.textContent,
    ).toBe("Could not open the macOS folder picker.");
  });
});
