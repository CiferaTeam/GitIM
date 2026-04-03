import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function joinChannelCommand(
  channel: string,
  options: { targets?: string[] }
): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.joinChannel(channel, options.targets);

  if (res.ok) {
    const who = options.targets?.length ? options.targets.join(', ') : '你';
    console.log(`${who} 已加入 #${channel}`);
  } else {
    console.error(`加入失败: ${res.error}`);
    process.exit(1);
  }
}
