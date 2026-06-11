import { describe, expect, it } from "vitest";
import { buildSidebarTree } from "./sidebar-tree";
import type { Channel, Project } from "./types";

function ch(name: string, project?: string | null): Channel {
  return {
    name,
    kind: "channel",
    unreadCount: 0,
    hasMention: false,
    members: ["alice"],
    created_by: "alice",
    project,
  };
}

function pr(slug: string): Project {
  return {
    slug,
    meta: {
      display_name: slug,
      created_by: "alice",
      created_at: "2026-01-01T00:00:00Z",
      introduction: "",
    },
    channel_count: 0, // ignored by tree algorithm
  };
}

describe("buildSidebarTree", () => {
  it("mixes channels and projects at top level", () => {
    const tree = buildSidebarTree(
      [ch("dev", "design"), ch("random"), ch("ml", "design")],
      [pr("design")],
      new Set(),
    );
    // "design" project slug < "random" channel name lexicographically
    expect(tree).toHaveLength(2);
    expect(tree[0]).toMatchObject({ kind: "project" });
    expect(tree[1]).toMatchObject({ kind: "channel" });
    if (tree[0].kind === "project") {
      expect(tree[0].children).toHaveLength(2);
    }
  });

  it("hides empty project", () => {
    const tree = buildSidebarTree([ch("random")], [pr("design")], new Set());
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: "channel" });
  });

  it("pinned items float to top", () => {
    const tree = buildSidebarTree(
      [ch("a"), ch("b"), ch("z")],
      [],
      new Set(["channel:z"]),
    );
    const names = tree.map((n) => (n.kind === "channel" ? n.channel.name : ""));
    expect(names).toEqual(["z", "a", "b"]);
  });

  it("orphan channel.project (project deleted) falls to unassigned", () => {
    const tree = buildSidebarTree([ch("dev", "ghost-project")], [], new Set());
    expect(tree).toHaveLength(1);
    expect(tree[0]).toMatchObject({ kind: "channel" });
  });

  it("children inside project sorted by channel name", () => {
    const tree = buildSidebarTree(
      [ch("zee", "design"), ch("alpha", "design")],
      [pr("design")],
      new Set(),
    );
    expect(tree).toHaveLength(1);
    if (tree[0].kind === "project") {
      expect(tree[0].children.map((c) => c.name)).toEqual(["alpha", "zee"]);
    }
  });

  it("all unassigned channels when no projects", () => {
    const tree = buildSidebarTree(
      [ch("general"), ch("dev"), ch("random")],
      [],
      new Set(),
    );
    expect(tree).toHaveLength(3);
    expect(tree.every((n) => n.kind === "channel")).toBe(true);
    // sorted lexicographically
    const names = tree.map((n) => (n.kind === "channel" ? n.channel.name : ""));
    expect(names).toEqual(["dev", "general", "random"]);
  });

  it("multiple projects sorted by slug when no pins", () => {
    const tree = buildSidebarTree(
      [ch("a", "zebra"), ch("b", "apple")],
      [pr("zebra"), pr("apple")],
      new Set(),
    );
    // "apple" < "zebra" lexicographically
    expect(tree).toHaveLength(2);
    expect(tree[0].kind === "project" && tree[0].project.slug).toBe("apple");
    expect(tree[1].kind === "project" && tree[1].project.slug).toBe("zebra");
  });

  it("pinned project floats above unpinned channel even with earlier letter", () => {
    const tree = buildSidebarTree(
      [ch("aaa"), ch("b", "zzz-proj")],
      [pr("zzz-proj")],
      new Set(["project:zzz-proj"]),
    );
    // "zzz-proj" is pinned → comes before "aaa"
    expect(tree[0]).toMatchObject({ kind: "project" });
    expect(tree[1]).toMatchObject({ kind: "channel" });
  });
});
