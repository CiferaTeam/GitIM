import { GitimClient } from '../client.js';
import { ensureDaemon, findRepoRoot } from '../daemon.js';

export async function reindexCommand() {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repo');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);

  console.log('Rebuilding search index...');
  const result = await client.reindex();

  if (!result.ok) {
    console.error(`Reindex failed: ${result.error}`);
    process.exit(1);
  }

  console.log(`Done. ${result.data.messages_indexed} messages indexed.`);
}
