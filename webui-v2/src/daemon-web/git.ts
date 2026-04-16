import git, { type AuthCallback, type ProgressCallback } from "isomorphic-git";
import http from "isomorphic-git/http/web";
import { getFs } from "./storage";

/** Clone a remote repo into the IndexedDB filesystem. */
export async function cloneRepo(
  url: string,
  dir: string,
  corsProxy: string,
  onAuth: AuthCallback,
  onProgress?: ProgressCallback,
): Promise<void> {
  await git.clone({
    fs: getFs(),
    http,
    dir,
    url,
    corsProxy,
    onAuth,
    onProgress,
    singleBranch: true,
    depth: 50,
  });
}

/** Fetch latest objects from origin. */
export async function fetchOrigin(
  dir: string,
  corsProxy: string,
  onAuth: AuthCallback,
): Promise<void> {
  await git.fetch({
    fs: getFs(),
    http,
    dir,
    corsProxy,
    onAuth,
    remote: "origin",
    singleBranch: true,
  });
}

/** Resolve HEAD to a commit SHA. */
export async function resolveHead(dir: string): Promise<string> {
  return git.resolveRef({ fs: getFs(), dir, ref: "HEAD" });
}

/** Resolve the remote HEAD (origin's default branch) to a commit SHA. */
export async function resolveRemoteHead(dir: string): Promise<string> {
  const branches = await git.listBranches({
    fs: getFs(),
    dir,
    remote: "origin",
  });
  const branch = branches.includes("main")
    ? "main"
    : branches.filter((b) => b !== "HEAD")[0] ?? "main";
  return git.resolveRef({
    fs: getFs(),
    dir,
    ref: `refs/remotes/origin/${branch}`,
  });
}

/** Stage files and create a commit. Returns the new commit SHA. */
export async function addAndCommit(
  dir: string,
  filepaths: string[],
  message: string,
  author: string,
): Promise<string> {
  const fs = getFs();
  for (const filepath of filepaths) {
    await git.add({ fs, dir, filepath });
  }
  return git.commit({
    fs,
    dir,
    message,
    author: { name: author, email: `${author}@gitim` },
  });
}

/** Push local commits to origin. */
export async function push(
  dir: string,
  corsProxy: string,
  onAuth: AuthCallback,
): Promise<void> {
  await git.push({
    fs: getFs(),
    http,
    dir,
    corsProxy,
    onAuth,
    remote: "origin",
  });
}

/** Checkout a ref (branch name or commit SHA). */
export async function checkout(dir: string, ref: string): Promise<void> {
  await git.checkout({ fs: getFs(), dir, ref });
}

/** Return the list of file paths that differ between two commits. */
export async function diffTrees(
  dir: string,
  commitHash1: string,
  commitHash2: string,
): Promise<string[]> {
  const fs = getFs();
  const changed: string[] = [];

  await git.walk({
    fs,
    dir,
    trees: [git.TREE({ ref: commitHash1 }), git.TREE({ ref: commitHash2 })],
    map: async (filepath, [entryA, entryB]) => {
      if (filepath === ".") return undefined;

      const [oidA, oidB] = await Promise.all([
        entryA?.oid(),
        entryB?.oid(),
      ]);

      if (oidA !== oidB) {
        changed.push(filepath);
      }
      return undefined;
    },
  });

  return changed;
}

/** Hard reset local HEAD and working tree to match a remote ref's commit. */
export async function resetToRemote(
  dir: string,
  remoteRef: string,
): Promise<void> {
  const fs = getFs();
  const commit = await git.resolveRef({ fs, dir, ref: remoteRef });

  // Point the current branch at the remote commit
  const branch = await getCurrentBranch(dir);
  await git.writeRef({
    fs,
    dir,
    ref: `refs/heads/${branch}`,
    value: commit,
    force: true,
  });

  await git.checkout({ fs, dir, ref: branch, force: true });
}

/** Get the current branch name, defaulting to 'main'. */
export async function getCurrentBranch(dir: string): Promise<string> {
  const branch = await git.currentBranch({ fs: getFs(), dir });
  return branch ?? "main";
}
