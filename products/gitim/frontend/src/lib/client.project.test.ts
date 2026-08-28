// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./backend", () => ({
  HttpBackend: class {},
  LocalBackend: class {},
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({ default: class {} }));

vi.mock("./local-network-fetch", () => ({
  localNetworkFetch: vi.fn(),
}));

import {
  listProjects,
  validateProjectIntroduction,
  validateProjectName,
  validateProjectSlug,
} from "./client";
import { localNetworkFetch } from "./local-network-fetch";
import { useConnectionStore } from "../hooks/use-connection-store";

beforeEach(() => {
  useConnectionStore.setState({ mode: "remote", port: 16868 });
  vi.mocked(localNetworkFetch).mockReset();
});

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

describe("project metadata validation", () => {
  it("enforces the daemon byte limits", () => {
    expect(validateProjectName("a".repeat(64))).toBeNull();
    expect(validateProjectName("a".repeat(65))).toBe(
      "Project name must be at most 64 UTF-8 bytes",
    );
    expect(validateProjectName("界".repeat(22))).toBe(
      "Project name must be at most 64 UTF-8 bytes",
    );
    expect(validateProjectIntroduction("a".repeat(500))).toBeNull();
    expect(validateProjectIntroduction("a".repeat(501))).toBe(
      "Introduction must be at most 500 UTF-8 bytes",
    );
  });
});

describe("listProjects", () => {
  it("rejects failed API envelopes instead of replacing project state with empty", async () => {
    vi.mocked(localNetworkFetch).mockResolvedValue({
      json: async () => ({ ok: false, error: "daemon unavailable" }),
    } as Response);

    await expect(listProjects("room")).rejects.toThrow("daemon unavailable");
  });
});
