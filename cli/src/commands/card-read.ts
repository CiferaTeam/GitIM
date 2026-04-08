import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function cardReadCommand(board: string, cardId: string, options: { limit?: string; since?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const limit = options.limit ? parseInt(options.limit, 10) : undefined;
  const since = options.since ? parseInt(options.since, 10) : undefined;
  const res = await client.readCard(board, cardId, limit, since);
  if (res.ok) {
    const entries = res.data?.entries ?? [];
    for (const e of entries) {
      if (e.type === 'message') {
        const reply = e.point_to > 0 ? ` (re: L${e.point_to})` : '';
        console.log(`[L${e.line_number}] @${e.author} ${e.timestamp}${reply}`);
        console.log(`  ${e.body}`);
      }
    }
  } else { console.error('Error:', res.error); process.exit(1); }
}
