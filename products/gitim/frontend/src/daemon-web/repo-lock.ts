let repoLock: Promise<void> = Promise.resolve();

/** Serialize browser-repo mutations.
 *
 * The web worker can handle overlapping async RPCs while the background sync
 * loop is also running. A reset-to-remote must not interleave with a local
 * write+commit, or the just-written commit can be orphaned from the branch.
 */
export async function withRepoLock<T>(fn: () => Promise<T>): Promise<T> {
  const previous = repoLock;
  let release!: () => void;
  repoLock = new Promise<void>((resolve) => {
    release = resolve;
  });

  try {
    await previous.catch(() => undefined);
    return await fn();
  } finally {
    release();
  }
}
