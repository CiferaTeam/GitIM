// Conflict resolution for browser sync.
//
// The substantive work — parsing, renumbering local additions after the
// remote's last line, and building the rebase commit message — is the Rust
// pure functions `resolve_content_pure` / `build_rebase_commit_msg`, reached
// through wasm. This module is just the IO-shape orchestration around them.
//
// `extractThreadAdditions` has no Rust pure-fn equivalent: the native daemon
// computes additions from the filesystem inside `resolve_content`. In the
// browser we derive them from already-read content (base/local), so this
// append-only diff stays here as TS glue. It does string work only — no
// .thread parsing — so it needs no wasm.

import {
  resolveContentPure,
  buildRebaseCommitMsg,
} from "gitim-wasm";

export interface ResolveResult {
  files: Record<string, string>;
  commitMessage: string;
}

// Shape of gitim-sync's ResolvedFile / RenumberMapping as serialized by wasm.
interface ResolvedFile {
  path: string;
  content: string;
}
interface RenumberMapping {
  file: string;
  old_line: number;
  new_line: number;
}
interface ResolveContentResult {
  files: ResolvedFile[];
  mappings: RenumberMapping[];
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
  const additionsJson = JSON.stringify(localAdditions);
  const { files, mappings } = resolveContentPure(
    additionsJson,
    JSON.stringify(remoteContents),
  ) as ResolveContentResult;

  const fileMap: Record<string, string> = {};
  for (const f of files) {
    fileMap[f.path] = f.content;
  }

  const commitMessage = buildRebaseCommitMsg(
    JSON.stringify(mappings),
    additionsJson,
  );

  return { files: fileMap, commitMessage };
}
