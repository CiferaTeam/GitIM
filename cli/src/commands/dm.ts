import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function dmSendCommand(handler: string, body: string, options: { author: string; replyTo?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const channel = `dm:${options.author},${handler}`;
  const replyTo = options.replyTo ? parseInt(options.replyTo, 10) : undefined;
  const res = await client.send(channel, body, options.author, replyTo);

  if (res.ok) {
    console.log('DM sent.');
  } else {
    console.error('Error:', res.error);
  }
}

export async function dmReadCommand(handler: string, options: { author: string; limit?: string; since?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const channel = `dm:${options.author},${handler}`;
  const limit = options.limit ? parseInt(options.limit, 10) : undefined;
  const since = options.since ? parseInt(options.since, 10) : undefined;
  const res = await client.read(channel, limit, since);

  if (res.ok) {
    console.log(JSON.stringify(res.data, null, 2));
  } else {
    console.error('Error:', res.error);
  }
}

export async function dmListCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  // List DM files by scanning dm/ directory
  const fs = await import('node:fs');
  const path = await import('node:path');
  const dmDir = path.join(repoRoot, 'dm');

  if (!fs.existsSync(dmDir)) {
    console.log('No DM conversations.');
    return;
  }

  const files = fs.readdirSync(dmDir).filter((f: string) => f.endsWith('.thread'));
  const conversations = files.map((f: string) => f.replace('.thread', ''));

  if (conversations.length === 0) {
    console.log('No DM conversations.');
  } else {
    for (const conv of conversations) {
      console.log(conv);
    }
  }
}
