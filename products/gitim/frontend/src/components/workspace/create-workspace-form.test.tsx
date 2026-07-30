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

function setInputValue(input: HTMLInputElement, value: string) {
  const valueSetter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )?.set;
  valueSetter?.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

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

  it("enables manual path entry when the native picker is unavailable", async () => {
    vi.mocked(client.pickWorkspaceDirectory).mockResolvedValue({
      ok: false,
      error: "Native folder selection is available on macOS.",
      error_code: "directory_picker_unavailable",
    });

    act(() => {
      root.render(<CreateWorkspaceForm />);
    });

    const pathInput = container.querySelector<HTMLInputElement>(
      "[data-testid='ws-path']",
    );
    expect(pathInput?.readOnly).toBe(true);

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>("[data-testid='ws-folder-picker']")
        ?.click();
      await Promise.resolve();
    });

    expect(pathInput?.readOnly).toBe(false);
    expect(container.querySelector("[data-testid='ws-create-error']")).toBeNull();
    expect(container.textContent).toContain(
      "Enter the absolute path to an existing workspace folder.",
    );

    act(() => {
      setInputValue(pathInput!, "/srv/workspaces/team-linux");
    });
    expect(pathInput?.value).toBe("/srv/workspaces/team-linux");
    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-slug']"),
    ).toHaveProperty("value", "team-linux");
  });

  it("offers manual path entry without opening the picker", () => {
    act(() => {
      root.render(<CreateWorkspaceForm />);
    });

    act(() => {
      container
        .querySelector<HTMLButtonElement>("[data-testid='ws-manual-path']")
        ?.click();
    });

    expect(client.pickWorkspaceDirectory).not.toHaveBeenCalled();
    expect(
      container.querySelector<HTMLInputElement>("[data-testid='ws-path']")
        ?.readOnly,
    ).toBe(false);
  });

  it("recovers from an unexpected picker rejection", async () => {
    vi.mocked(client.pickWorkspaceDirectory).mockRejectedValue(
      new Error("picker bridge stopped"),
    );

    act(() => {
      root.render(<CreateWorkspaceForm />);
    });

    await act(async () => {
      container
        .querySelector<HTMLButtonElement>("[data-testid='ws-folder-picker']")
        ?.click();
      await Promise.resolve();
      await Promise.resolve();
    });

    const picker = container.querySelector<HTMLButtonElement>(
      "[data-testid='ws-folder-picker']",
    );
    expect(picker?.disabled).toBe(false);
    expect(picker?.textContent).toContain("Choose folder");
    expect(
      container.querySelector("[data-testid='ws-create-error']")?.textContent,
    ).toBe("picker bridge stopped");
  });
});
