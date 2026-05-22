import { describe, expect, it } from "vitest";
import { formatDmDisplayName } from "./dm-display-name";

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
