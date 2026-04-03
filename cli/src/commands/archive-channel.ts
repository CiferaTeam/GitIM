import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function archiveChannelCommand(name: string): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.archiveChannel(name);

  if (res.ok) {
    console.log(`频道 #${name} 已归档`);
  } else {
    console.error(`归档失败: ${res.error}`);
    process.exit(1);
  }
}
