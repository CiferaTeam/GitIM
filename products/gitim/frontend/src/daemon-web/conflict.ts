// Pure conflict resolution — TS equivalent of gitim-sync's resolve_content_pure.
// Renumbers local additions so they follow the remote's max line number,
// then appends them to the remote content.

import { parseThread } from "./parser";
import { formatMessage, formatEvent } from "./formatter";

export interface ResolveResult {
  files: Record<string, string>;
  commitMessage: string;
}

export function extractThreadAdditions(
  filePath: string,
  localContent: string,
  baseContent: string | null,
): string {
  if (!filePath.endsWith(".thread")) {
    throw new Error(`Cannot auto-merge non-thread browser sync conflict: ${filePath}`);
  }
  if (!baseContent) return localContent;
  if (localContent === baseContent) return "";
  if (!localContent.startsWith(baseContent)) {
    throw new Error(`Cannot auto-merge non-append thread conflict: ${filePath}`);
  }
  return localContent.slice(baseContent.length);
}

export function resolveConflicts(
  localAdditions: Record<string, string>,
  remoteContents: Record<string, string>,
): ResolveResult {
  const files: Record<string, string> = {};
  const messageParts: string[] = [];

  for (const [filePath, localContent] of Object.entries(localAdditions)) {
    const remote = remoteContents[filePath] ?? "";

    // Find the highest line number in the remote content
    let maxLine = 0;
    if (remote) {
      const remoteFile = parseThread(remote);
      for (const entry of remoteFile.entries) {
        if (entry.line_number > maxLine) maxLine = entry.line_number;
      }
    }

    // Parse local additions and build a renumbering map
    const localFile = parseThread(localContent);
    const lineMap = new Map<number, number>();
    let nextLine = maxLine + 1;

    for (const entry of localFile.entries) {
      lineMap.set(entry.line_number, nextLine);
      nextLine++;
    }

    // Renumber entries, preserving parent references where possible
    let renumbered = "";
    for (const entry of localFile.entries) {
      const newLn = lineMap.get(entry.line_number)!;

      if (entry.type === "message") {
        const newPt =
          entry.point_to === 0
            ? 0
            : lineMap.get(entry.point_to) ?? entry.point_to;
        renumbered += formatMessage(
          newLn,
          newPt,
          entry.author,
          entry.timestamp,
          entry.body,
        );
      } else {
        renumbered += formatEvent(
          newLn,
          entry.author,
          entry.timestamp,
          entry.event_type,
          entry.meta,
        );
      }
    }

    // Merge: remote content + renumbered local appended
    let merged = remote;
    if (merged && !merged.endsWith("\n")) merged += "\n";
    merged += renumbered;

    files[filePath] = merged;

    // Build commit message lines
    const channel = filePath
      .replace(/.*\//, "")
      .replace(/\.thread$/, "");
    for (const entry of localFile.entries) {
      const newLn = lineMap.get(entry.line_number)!;
      messageParts.push(
        `msg: @${entry.author} -> ${channel} L${String(newLn).padStart(6, "0")}(rebased)`,
      );
    }
  }

  return {
    files,
    commitMessage:
      messageParts.length > 0
        ? messageParts.join("\n")
        : "msg: sync after rebase",
  };
}
