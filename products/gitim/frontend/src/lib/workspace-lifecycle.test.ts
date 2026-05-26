import { afterEach, describe, expect, it, vi } from "vitest";
import {
  _resetWorkspaceLifecycleForTests,
  emitWorkspaceSwitch,
  onWorkspaceSwitch,
} from "./workspace-lifecycle";

describe("workspace-lifecycle", () => {
  afterEach(() => {
    _resetWorkspaceLifecycleForTests();
  });

  it("fires every registered listener on emit", () => {
    const a = vi.fn();
    const b = vi.fn();
    onWorkspaceSwitch(a);
    onWorkspaceSwitch(b);

    emitWorkspaceSwitch();

    expect(a).toHaveBeenCalledTimes(1);
    expect(b).toHaveBeenCalledTimes(1);
  });

  it("returns an unsubscribe that detaches the listener", () => {
    const fn = vi.fn();
    const off = onWorkspaceSwitch(fn);

    emitWorkspaceSwitch();
    off();
    emitWorkspaceSwitch();

    expect(fn).toHaveBeenCalledTimes(1);
  });

  it("isolates listener failures — a throwing listener can't block siblings", () => {
    const before = vi.fn();
    const boom = vi.fn(() => {
      throw new Error("listener exploded");
    });
    const after = vi.fn();
    onWorkspaceSwitch(before);
    onWorkspaceSwitch(boom);
    onWorkspaceSwitch(after);

    // Silence the expected console.error from the bus's try/catch.
    const errSpy = vi.spyOn(console, "error").mockImplementation(() => {});

    expect(() => emitWorkspaceSwitch()).not.toThrow();
    expect(before).toHaveBeenCalledTimes(1);
    expect(after).toHaveBeenCalledTimes(1);
    expect(errSpy).toHaveBeenCalled();

    errSpy.mockRestore();
  });

  it("dedupes a listener registered twice (Set semantics)", () => {
    const fn = vi.fn();
    onWorkspaceSwitch(fn);
    onWorkspaceSwitch(fn);

    emitWorkspaceSwitch();

    expect(fn).toHaveBeenCalledTimes(1);
  });
});
