import { beforeEach, describe, expect, it, vi } from "vitest";

const wipedFsNames = vi.hoisted((): string[] => []);
const wipeControls = vi.hoisted(() => ({
  hold: false,
  pending: [] as Array<{ name: string; resolve: () => void }>,
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({
  default: class MockLightningFS {
    promises: { stat: () => Promise<unknown> };

    constructor(name: string, options?: { wipe?: boolean }) {
      if (options?.wipe) {
        wipedFsNames.push(name);
        if (wipeControls.hold) {
          let resolve!: () => void;
          const promise = new Promise<void>((r) => {
            resolve = r;
          });
          wipeControls.pending.push({ name, resolve });
          this.promises = { stat: () => promise };
          return;
        }
      }
      this.promises = { stat: () => Promise.resolve({}) };
    }
  },
}));

import {
  clearAllBrowserWorkspaces,
  createBrowserWorkspace,
  forgetBrowserWorkspace,
  forgetBrowserWorkspaceAndWipeCache,
  getBrowserWorkspace,
  listBrowserWorkspaceSummaries,
  loadBrowserWorkspaces,
  loadSessionToken,
  migrateLegacyBrowserWorkspace,
  saveSessionToken,
  wipeAllBrowserWorkspaceCaches,
  wipeBrowserWorkspaceCache,
} from "./browser-workspaces";

describe("browser workspaces", () => {
  beforeEach(() => {
    localStorage.clear();
    sessionStorage.clear();
    wipedFsNames.length = 0;
    wipeControls.hold = false;
    wipeControls.pending.length = 0;
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

  it("lists legacy browser config through the v2 registry migration", () => {
    localStorage.setItem(
      "gitim-local-config",
      JSON.stringify({
        remoteUrl: "https://github.com/acme/legacy",
      }),
    );

    expect(listBrowserWorkspaceSummaries()).toEqual([
      expect.objectContaining({
        id: "legacy",
        slug: "browser-legacy",
        path: "indexeddb://gitim/repo",
      }),
    ]);
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

  it("forgetting legacy removes the legacy config so it does not reappear", () => {
    const legacy = migrateLegacyBrowserWorkspaceFromConfig();

    forgetBrowserWorkspace(legacy.id);

    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(localStorage.getItem("gitim-local-config")).toBeNull();
    expect(listBrowserWorkspaceSummaries()).toEqual([]);
  });

  it("wipes a workspace cache before forgetting the registry entry", async () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    await forgetBrowserWorkspaceAndWipeCache(ws.id);

    expect(wipedFsNames).toEqual([ws.storage.fsName]);
    expect(getBrowserWorkspace(ws.id)).toBeUndefined();
    expect(loadSessionToken(ws.id)).toBeUndefined();
  });

  it("gets a browser workspace by id or slug", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      workspaceName: "Phone",
    });

    expect(getBrowserWorkspace(ws.id)).toEqual(ws);
    expect(getBrowserWorkspace(ws.slug)).toEqual(ws);
  });

  it("wipes one workspace cache without clearing registry or token state", async () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      workspaceName: "Phone",
    });
    const other = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/other",
      workspaceName: "Other",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    await wipeBrowserWorkspaceCache(ws.slug);

    expect(wipedFsNames).toEqual([ws.storage.fsName]);
    expect(loadBrowserWorkspaces().map((workspace) => workspace.id)).toEqual([
      ws.id,
      other.id,
    ]);
    expect(loadSessionToken(ws.id)).toBe("github_pat_secret");
  });

  it("waits for the LightningFS wipe activation before resolving", async () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      workspaceName: "Phone",
    });
    wipeControls.hold = true;
    let resolved = false;

    const wipe = wipeBrowserWorkspaceCache(ws.slug).then(() => {
      resolved = true;
    });
    await Promise.resolve();

    expect(wipedFsNames).toEqual([ws.storage.fsName]);
    expect(wipeControls.pending.map((pending) => pending.name)).toEqual([
      ws.storage.fsName,
    ]);
    expect(resolved).toBe(false);

    wipeControls.pending[0].resolve();
    await wipe;

    expect(resolved).toBe(true);
  });

  it("wipes all registered workspace caches and the legacy cache once", async () => {
    const legacy = migrateLegacyBrowserWorkspaceFromConfig();
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");

    await wipeAllBrowserWorkspaceCaches();

    expect(wipedFsNames).toEqual([legacy.storage.fsName, ws.storage.fsName]);
    expect(loadBrowserWorkspaces()).toHaveLength(2);
    expect(loadSessionToken(ws.id)).toBe("github_pat_secret");
  });

  it("clears all registry and token state", () => {
    const ws = createBrowserWorkspace({
      remoteUrl: "https://github.com/acme/room",
      corsProxy: "https://proxy.example",
      handler: "flame4",
      workspaceName: "Phone",
    });
    saveSessionToken(ws.id, "github_pat_secret");
    localStorage.setItem(
      "gitim-local-config",
      JSON.stringify({ remoteUrl: "https://github.com/acme/legacy" }),
    );

    clearAllBrowserWorkspaces();

    expect(loadBrowserWorkspaces()).toEqual([]);
    expect(loadSessionToken(ws.id)).toBeUndefined();
    expect(localStorage.getItem("gitim-local-config")).toBeNull();
  });
});

function migrateLegacyBrowserWorkspaceFromConfig() {
  localStorage.setItem(
    "gitim-local-config",
    JSON.stringify({
      remoteUrl: "https://github.com/acme/legacy",
      corsProxy: "https://cors.isomorphic-git.org",
    }),
  );
  const legacy = migrateLegacyBrowserWorkspace();
  if (!legacy) throw new Error("expected legacy browser workspace");
  return legacy;
}
