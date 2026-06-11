import type { Channel, Project } from "./types";

export type SidebarNode =
  | { kind: "channel"; channel: Channel }
  | {
      kind: "project";
      project: Project;
      children: Channel[];
    };

/**
 * Build a flat mixed-sort sidebar list from channels + projects.
 *
 * Rules:
 * - Channel with no project (or orphan project ref) → top-level SidebarNode.channel
 * - Project with at least one assigned channel → SidebarNode.project containing its channels
 * - Empty project (zero assigned channels) → hidden (not emitted)
 * - Top-level sort: pinned items first, then lexicographic by label
 *   (channel: channel.name; project: project.slug)
 * - Children inside a project node: sorted by channel.name lexicographically
 *
 * Pin keys:
 *   channel: `channel:${channel.name}`
 *   project: `project:${project.slug}`
 */
export function buildSidebarTree(
  channels: Channel[],
  projects: Project[],
  pinnedKeys: Set<string>,
): SidebarNode[] {
  const projectsBySlug = new Map(projects.map((p) => [p.slug, p]));
  const childrenByProject = new Map<string, Channel[]>();
  const unassigned: Channel[] = [];

  for (const ch of channels) {
    const proj = ch.project;
    if (proj && projectsBySlug.has(proj)) {
      const existing = childrenByProject.get(proj);
      if (existing) {
        existing.push(ch);
      } else {
        childrenByProject.set(proj, [ch]);
      }
    } else {
      // null / undefined, or orphan project ref (project deleted) → unassigned
      unassigned.push(ch);
    }
  }

  // Sort children inside each project by channel name
  for (const list of childrenByProject.values()) {
    list.sort((a, b) => a.name.localeCompare(b.name));
  }

  const nodes: SidebarNode[] = [];

  // Emit only non-empty projects (in projects array order)
  for (const proj of projects) {
    const children = childrenByProject.get(proj.slug);
    if (!children || children.length === 0) continue;
    nodes.push({ kind: "project", project: proj, children });
  }

  // Emit unassigned channels
  for (const ch of unassigned) {
    nodes.push({ kind: "channel", channel: ch });
  }

  // Sort top level: pinned first, then lexicographic label
  function keyOf(n: SidebarNode): string {
    return n.kind === "channel"
      ? `channel:${n.channel.name}`
      : `project:${n.project.slug}`;
  }

  function labelOf(n: SidebarNode): string {
    return n.kind === "channel" ? n.channel.name : n.project.slug;
  }

  nodes.sort((a, b) => {
    const aP = pinnedKeys.has(keyOf(a));
    const bP = pinnedKeys.has(keyOf(b));
    if (aP !== bP) return aP ? -1 : 1;
    return labelOf(a).localeCompare(labelOf(b));
  });

  return nodes;
}
