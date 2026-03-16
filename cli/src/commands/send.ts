import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function sendCommand(channel: string, body: string, options: { author: string; replyTo?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const replyTo = options.replyTo ? parseInt(options.replyTo, 10) : undefined;
  const res = await client.send(channel, body, options.author, replyTo);

  if (res.ok) {
    console.log('Message sent.');
  } else {
    console.error('Error:', res.error);
  }
}
