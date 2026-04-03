import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function archivedChannelsCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.listArchivedChannels();

  if (res.ok) {
    const channels = res.data.channels as Array<{ name: string; kind: string; members: string[] }>;
    if (channels.length === 0) {
      console.log('暂无已归档频道');
    } else {
      for (const ch of channels) {
        console.log(`#${ch.name}`);
      }
    }
  } else {
    console.error('Error:', res.error);
  }
}
