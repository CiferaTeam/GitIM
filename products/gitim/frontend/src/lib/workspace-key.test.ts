import { describe, expect, it } from "vitest";
import {
  activeWorkspaceStorageKey,
  cursorWorkspaceKey,
  workspaceIdentity,
} from "./workspace-key";
import type { WorkspaceSummary } from "./types";

describe("workspace keys", () => {
  it("uses browser workspace id for local browser identity", () => {
    const ws: WorkspaceSummary = {
      id: "ws_abc123",
      slug: "browser-ws-abc123",
      workspace_name: "Phone",
      path: "indexeddb://gitim-ws-ws_abc123/repo",
      provider: "github",
      initialized: true,
      browser: true,
    };

    expect(workspaceIdentity("local", ws)).toBe("browser:ws_abc123");
    expect(cursorWorkspaceKey("local", ws)).toBe("gitim:cursor:browser:ws_abc123");
  });

  it("keeps runtime slugs isolated from browser ids", () => {
    const ws: WorkspaceSummary = {
      slug: "mobile",
      workspace_name: "Mobile",
      path: "/tmp/mobile",
      provider: "local",
      initialized: true,
    };

    expect(workspaceIdentity("remote", ws)).toBe("runtime:mobile");
    expect(activeWorkspaceStorageKey("remote")).toBe("gitim-active-workspace");
    expect(activeWorkspaceStorageKey("local")).toBe("gitim-active-browser-workspace");
  });
});
