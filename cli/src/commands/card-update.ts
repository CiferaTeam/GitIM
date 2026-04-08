import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function cardUpdateCommand(board: string, cardId: string, options: { status?: string; assignee?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.updateCard(board, cardId, options.status, options.assignee);
  if (res.ok) { console.log(`卡片 ${cardId} 已更新: status=${res.data.status}, assignee=${res.data.assignee ?? 'none'}`); }
  else { console.error(`更新失败: ${res.error}`); process.exit(1); }
}
