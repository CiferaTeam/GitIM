// @vitest-environment jsdom

import { describe, expect, it, vi } from "vitest";

vi.mock("./backend", () => ({
  HttpBackend: class {},
  LocalBackend: class {},
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({ default: class {} }));

import { validateProjectSlug } from "./client";

describe("validateProjectSlug", () => {
  it("accepts the project slug contract", () => {
    expect(validateProjectSlug("design")).toBeNull();
    expect(validateProjectSlug("team-2")).toBeNull();
  });

  it("rejects invalid and reserved project slugs", () => {
    expect(validateProjectSlug("")).toBe("Project slug is required");
    expect(validateProjectSlug("Design")).toBe(
      "Only lowercase letters, numbers, and hyphens",
    );
    expect(validateProjectSlug("team--2")).toBe(
      "Cannot contain consecutive hyphens",
    );
    expect(validateProjectSlug("channels")).toBe(
      '"channels" is reserved',
    );
  });
});
