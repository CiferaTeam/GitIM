import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function boardCreateCommand(name: string, options: { displayName?: string; statuses?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const statuses = options.statuses ? options.statuses.split(',').map(s => s.trim()) : undefined;
  const res = await client.createBoard(name, options.displayName, statuses);
  if (res.ok) { console.log(`看板 #${name} 创建成功`); }
  else { console.error(`创建失败: ${res.error}`); process.exit(1); }
}
