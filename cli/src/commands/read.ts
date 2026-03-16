import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function readCommand(channel: string, options: { limit?: string; since?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const limit = options.limit ? parseInt(options.limit, 10) : undefined;
  const since = options.since ? parseInt(options.since, 10) : undefined;
  const res = await client.read(channel, limit, since);

  if (res.ok) {
    console.log(JSON.stringify(res.data, null, 2));
  } else {
    console.error('Error:', res.error);
  }
}
