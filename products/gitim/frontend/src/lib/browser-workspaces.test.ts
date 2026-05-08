import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  clearAllBrowserWorkspaces,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  getBrowserWorkspace,
  listBrowserWorkspaceSummaries,
  loadBrowserWorkspaces,
  loadSessionToken,
  migrateLegacyBrowserWorkspace,
  saveSessionToken,
} from "./browser-workspaces";

describe("browser workspaces", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-05-08T12:00:00Z"));
  });

  it("creates isolated v2 workspaces", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });

    expect(ws.id).toMatch(/^ws_/);
    expect(ws.slug).toBe(`browser-${ws.id}`);
    expect(ws.storage.fsName).toBe(`gitim-ws-${ws.id}`);
    expect(ws.storage.repoDir).toBe("/repo");
    expect(listBrowserWorkspaceSummaries()).toEqual([
      expect.objectContaining({
        id: ws.id,
        slug: ws.slug,
        workspace_name: "Phone",
        path: `indexeddb://gitim-ws-${ws.id}/repo`,
        browser: true,
        needs_token: true,
      }),
    ]);
  });

  it("migrates the legacy single browser config without moving storage", () => {
    localStorage.setItem(
      "gitim-local-config",
      JSON.stringify({
        remoteUrl: "https://github.com/acme/legacy",
        corsProxy: "https://cors.isomorphic-git.org",
      }),
    );

    const legacy = migrateLegacyBrowserWorkspace();

    expect(legacy).toEqual(expect.objectContaining({
      id: "legacy",
      slug: "browser-legacy",
      workspace_name: "Browser",
      remoteUrl: "https://github.com/acme/legacy",
      storage: {
        fsName: "gitim",
        repoDir: "/repo",
        legacy: true,
      },
    }));
    expect(loadBrowserWorkspaces()).toHaveLength(1);
  });

  it("keeps session tokens outside the registry", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });

    saveSessionToken(ws.id, "github_pat_secret");

    expect(loadSessionToken(ws.id)).toBe("github_pat_secret");
    expect(localStorage.getItem("gitim-browser-workspaces-v2")).not.toContain("github_pat_secret");
  });

  it("forgets a workspace and removes its session token", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    forgetBrowserWorkspace(ws.id);

    expect(getBrowserWorkspace(ws.id)).toBeUndefined();
    expect(loadSessionToken(ws.id)).toBeUndefined();
  });

  it("clears all registry and token state", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    clearAllBrowserWorkspaces();

    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(loadSessionToken(ws.id)).toBeUndefined();
  });
});
