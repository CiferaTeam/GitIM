import { describe, expect, it } from "vitest";
import { KIND_STYLES, kindStyle } from "./kind-styles";

describe("kind-styles", () => {
  it("uses the same opacity convention across all three kinds", () => {
    // The opacity convention is `bg-X/15, border-X/30, text-X` — chosen
    // to match agent-status.tsx, the closest existing precedent. If a
    // future palette pass changes one, it MUST change all three or the
    // chips drift visually. This test makes the drift loud.
    const opacities = (s: string) => s.match(/\/\d+/g) ?? [];
    for (const kind of ["past", "future", "missed"] as const) {
      const op = opacities(KIND_STYLES[kind].chip).sort();
      expect(op, `kind ${kind} chip opacities`).toEqual(["/15", "/30"]);
    }
  });

  it("returns the missed style for an unknown kind (defensive fallback)", () => {
    // A future runtime sending `kind: "failed"` (mentioned in design v2
    // notes) must not crash the calendar — kindStyle returns a safe
    // default. Picked `missed` because (a) it's the most attention-
    // getting and (b) an unknown status is closer to "didn't go well"
    // than to "succeeded" or "scheduled".
    const fallback = kindStyle("failed");
    expect(fallback).toBe(KIND_STYLES.missed);
    expect(fallback.label).toBe("未执行");
  });

  it("returns the right style for each known kind", () => {
    expect(kindStyle("past").label).toBe("已执行");
    expect(kindStyle("future").label).toBe("未来");
    expect(kindStyle("missed").label).toBe("未执行");
  });
});
