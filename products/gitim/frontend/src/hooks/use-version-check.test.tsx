// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { ApiResponse } from "@/lib/types";
import type { UpdateAndRestartData } from "@/lib/client";

const mocks = vi.hoisted(() => ({
  checkVersion: vi.fn(),
  health: vi.fn(),
  updateAndRestart: vi.fn(),
  toast: {
    error: vi.fn(),
    info: vi.fn(),
    success: vi.fn(),
  },
}));

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

vi.mock("../lib/gitim-api", () => ({
  checkVersion: mocks.checkVersion,
}));

vi.mock("../lib/client", () => ({
  health: mocks.health,
  updateAndRestart: mocks.updateAndRestart,
}));

vi.mock("sonner", () => ({
  toast: mocks.toast,
}));

import { useVersionCheck } from "./use-version-check";
import { useConnectionStore } from "./use-connection-store";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function Probe({ onValue }: { onValue: (value: ReturnType<typeof useVersionCheck>) => void }) {
  const value = useVersionCheck();
  onValue(value);
  return <button type="button">{value.isUpdating ? "updating" : "idle"}</button>;
}

describe("useVersionCheck", () => {
  let root: Root | null = null;

  beforeEach(() => {
    testEnv.localStorage.clear();
    mocks.checkVersion.mockResolvedValue({ ok: true, latest_version: "0.6.0" });
    mocks.health.mockResolvedValue({ ok: true, data: { version: "0.5.0" } });
    useConnectionStore.setState({
      mode: "remote",
      port: 5317,
      runtimeVersion: "0.5.0",
      isUpdating: false,
      isRestarting: false,
      updateError: null,
    });
  });

  afterEach(() => {
    if (root) {
      root.unmount();
      root = null;
    }
    document.body.innerHTML = "";
    vi.clearAllMocks();
  });

  it("enters updating state immediately while the update request is still pending", async () => {
    const accept = deferred<ApiResponse<UpdateAndRestartData>>();
    mocks.updateAndRestart.mockReturnValue(accept.promise);
    let hookValue: ReturnType<typeof useVersionCheck> | null = null;

    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    await act(async () => {
      root?.render(<Probe onValue={(value) => { hookValue = value; }} />);
      await Promise.resolve();
    });

    let updatePromise: Promise<void> | undefined;
    await act(async () => {
      updatePromise = hookValue?.triggerUpdate();
      await Promise.resolve();
    });

    expect(mocks.updateAndRestart).toHaveBeenCalledTimes(1);
    expect(useConnectionStore.getState().isUpdating).toBe(true);

    accept.resolve({ ok: false, error: "cancelled" });
    await act(async () => {
      await updatePromise;
    });
  });
});
