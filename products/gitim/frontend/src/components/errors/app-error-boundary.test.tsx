// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { AppErrorBoundary } from "./app-error-boundary";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

describe("AppErrorBoundary", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.restoreAllMocks();
  });

  it("reports an uncaught render error and recovers when the user retries", async () => {
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    let shouldThrow = true;

    function ThrowUntilRetry() {
      if (shouldThrow) {
        throw new Error("render exploded");
      }
      return <div>Recovered workspace</div>;
    }

    await act(async () => {
      root.render(
        <AppErrorBoundary>
          <ThrowUntilRetry />
        </AppErrorBoundary>,
      );
    });

    expect(container.querySelector('[role="alert"]')?.textContent).toContain(
      "Something went wrong",
    );
    expect(consoleError).toHaveBeenCalledWith(
      "[gitim] Unhandled UI error",
      expect.objectContaining({ message: "render exploded" }),
      expect.stringContaining("ThrowUntilRetry"),
    );

    shouldThrow = false;
    await act(async () => {
      container
        .querySelector<HTMLButtonElement>('button[data-action="retry"]')
        ?.click();
    });

    expect(container.textContent).toContain("Recovered workspace");
    expect(container.querySelector('[role="alert"]')).toBeNull();
  });
});
