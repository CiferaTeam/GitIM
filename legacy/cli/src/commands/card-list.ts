import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function cardListCommand(board: string, options: { status?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.listCards(board, options.status);
  if (res.ok) {
    const cards = res.data?.cards ?? [];
    if (cards.length === 0) { console.log('没有卡片'); return; }
    for (const c of cards) {
      const assignee = c.assignee ? `@${c.assignee}` : '';
      console.log(`[${c.status}] ${c.card_id}  ${c.title}  ${assignee}`);
    }
  } else { console.error('Error:', res.error); process.exit(1); }
}
