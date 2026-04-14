import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function cardSendCommand(board: string, cardId: string, body: string, options: { replyTo?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) { console.error('Not in a GitIM repository'); process.exit(1); }
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const replyTo = options.replyTo ? parseInt(options.replyTo, 10) : undefined;
  const res = await client.sendCardMessage(board, cardId, body, replyTo);
  if (res.ok) { console.log('Message sent.'); }
  else { console.error('Error:', res.error); process.exit(1); }
}
