import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function boardListCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const res = await client.listBoards();
  if (res.ok) {
    const boards = res.data?.boards ?? [];
    if (boards.length === 0) { console.log('没有看板'); return; }
    for (const b of boards) { console.log(`#${b.name}  ${b.display_name}  [${b.statuses.join(', ')}]`); }
  } else { console.error('Error:', res.error); process.exit(1); }
}
