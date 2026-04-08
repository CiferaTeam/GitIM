import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function cardCreateCommand(board: string, title: string, options: { assignee?: string; status?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.createCard(board, title, options.assignee, options.status);
  if (res.ok) { console.log(`卡片 ${res.data.card_id} 创建成功 (${board})`); }
  else { console.error(`创建失败: ${res.error}`); process.exit(1); }
}
