import fs from 'node:fs';
import path from 'node:path';
import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

function resolveAuthor(repoRoot: string, explicit?: string): string {
  if (explicit) return explicit;
  const mePath = path.join(repoRoot, '.gitim', 'me.json');
  if (fs.existsSync(mePath)) {
    const me = JSON.parse(fs.readFileSync(mePath, 'utf-8'));
    return me.handler;
  }
  console.error('Error: 未配置身份，请先运行 gitim onboard');
  process.exit(1);
}

export async function dmSendCommand(handler: string, body: string, options: { author?: string; replyTo?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  const author = resolveAuthor(repoRoot, options.author);
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const [h1, h2] = [author, handler].sort();
  const channel = `dm:${h1},${h2}`;
  const replyTo = options.replyTo ? parseInt(options.replyTo, 10) : undefined;
  const res = await client.send(channel, body, author, replyTo);

  if (res.ok) {
    console.log('DM sent.');
  } else {
    console.error('Error:', res.error);
  }
}

export async function dmReadCommand(handler: string, options: { author?: string; limit?: string; since?: string }): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  const author = resolveAuthor(repoRoot, options.author);
  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);
  const [h1, h2] = [author, handler].sort();
  const channel = `dm:${h1},${h2}`;
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
