import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function createChannelCommand(
  name: string,
  options: { displayName?: string; introduction?: string }
): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.createChannel(name, options.displayName, options.introduction);

  if (res.ok) {
    console.log(`频道 #${name} 创建成功`);
  } else {
    console.error(`创建失败: ${res.error}`);
    process.exit(1);
  }
}
