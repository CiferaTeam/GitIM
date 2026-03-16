import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function statusCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.status();

  if (res.ok) {
    console.log('Daemon status:', JSON.stringify(res.data, null, 2));
  } else {
    console.error('Error:', res.error);
  }
}
