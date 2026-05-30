import { describe, expect, it } from "vitest";
import { dmPeerHandler, formatDmDisplayName } from "./dm-display-name";

describe("formatDmDisplayName", () => {
  it("shows the other participant for current-user DMs", () => {
    expect(formatDmDisplayName("cfo--lewis", "lewis")).toBe("cfo");
    expect(formatDmDisplayName("lewis--planner", "lewis")).toBe("planner");
  });

  it("shows both participants for agent DMs outside the current user", () => {
    expect(formatDmDisplayName("cfo--glm51op2", "lewis")).toBe(
      "cfo ↔ glm51op2",
    );
  });

  it("keeps malformed names unchanged", () => {
    expect(formatDmDisplayName("general", "lewis")).toBe("general");
  });
});

describe("dmPeerHandler", () => {
  it("returns the peer handler for a 1:1 DM the current user is in", () => {
    expect(dmPeerHandler("cfo--lewis", "lewis")).toBe("cfo");
    expect(dmPeerHandler("lewis--planner", "lewis")).toBe("planner");
  });

  it("returns null when the current user isn't a participant", () => {
    expect(dmPeerHandler("cfo--glm51op2", "lewis")).toBeNull();
  });

  it("returns null for malformed (non-pair) names", () => {
    expect(dmPeerHandler("general", "lewis")).toBeNull();
  });

  it("resolves self-DMs to the user themselves", () => {
    expect(dmPeerHandler("lewis--lewis", "lewis")).toBe("lewis");
  });
});
