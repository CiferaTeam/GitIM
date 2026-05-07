import { describe, expect, it } from "vitest";
import { getVisibleNavigationItems } from "./navigation-items";

function labelsFor(
  mode: Parameters<typeof getVisibleNavigationItems>[0],
  surface: Parameters<typeof getVisibleNavigationItems>[1],
): string[] {
  return getVisibleNavigationItems(mode, surface).map((item) => item.label);
}

describe("getVisibleNavigationItems", () => {
  it("keeps Cards visible in browser mode on desktop", () => {
    expect(labelsFor("local", "desktop")).toEqual(["Chat", "Cards"]);
  });

  it("keeps Cards visible in browser mode on mobile", () => {
    expect(labelsFor("local", "mobile")).toEqual(["Chat", "Cards"]);
  });

  it("hides Agents from the mobile tab bar even with the runtime", () => {
    expect(labelsFor("remote", "mobile")).toEqual(["Chat", "Cards"]);
  });

  it("shows all primary tabs on runtime desktop", () => {
    expect(labelsFor("remote", "desktop")).toEqual(["Agents", "Chat", "Cards"]);
  });
});
