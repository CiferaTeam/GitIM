// Tests for the URL round-trip helpers (pure functions; no rendering).

import { describe, expect, it } from "vitest";

import { readFilterFromURL, writeFilterToURL } from "./card-filter-url";
import type { CardFilterState } from "./card-filter-bar";

const EMPTY: CardFilterState = {
  channels: [],
  labels: [],
  assignee: null,
  mineOnly: false,
  project: null,
};

describe("writeFilterToURL / readFilterFromURL — project round-trip", () => {
  it("round-trips a specific project slug", () => {
    const filter: CardFilterState = { ...EMPTY, project: "design" };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("design");
    const back = readFilterFromURL(params);
    expect(back.project).toBe("design");
  });

  it("round-trips the __unassigned__ sentinel", () => {
    const filter: CardFilterState = { ...EMPTY, project: "__unassigned__" };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("__unassigned__");
    const back = readFilterFromURL(params);
    expect(back.project).toBe("__unassigned__");
  });

  it("omits project param when project is null (All)", () => {
    const filter: CardFilterState = { ...EMPTY, project: null };
    const params = writeFilterToURL(filter);
    expect(params.has("project")).toBe(false);
  });

  it("reads null when project param is absent", () => {
    const params = new URLSearchParams();
    const back = readFilterFromURL(params);
    expect(back.project).toBeNull();
  });

  it("preserves other filter fields alongside project", () => {
    const filter: CardFilterState = {
      channels: ["dev"],
      labels: ["bug"],
      assignee: null,
      mineOnly: false,
      project: "infra",
    };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("infra");
    expect(params.getAll("channel")).toEqual(["dev"]);
    expect(params.getAll("label")).toEqual(["bug"]);
    const back = readFilterFromURL(params);
    expect(back.project).toBe("infra");
    expect(back.channels).toEqual(["dev"]);
    expect(back.labels).toEqual(["bug"]);
  });

  it("mineOnly round-trips with project filter present", () => {
    const filter: CardFilterState = {
      ...EMPTY,
      mineOnly: true,
      project: "design",
    };
    const params = writeFilterToURL(filter);
    expect(params.get("assignee")).toBe("__me__");
    expect(params.get("project")).toBe("design");
    const back = readFilterFromURL(params);
    expect(back.mineOnly).toBe(true);
    expect(back.assignee).toBeNull();
    expect(back.project).toBe("design");
  });
});
