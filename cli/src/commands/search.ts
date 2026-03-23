import { GitimClient } from '../client.js';
import { ensureDaemon, findRepoRoot } from '../daemon.js';

export async function searchCommand(query: string | undefined, options: {
  author?: string;
  channel?: string;
  type?: string;
  limit?: string;
  offset?: string;
}) {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repo');
    process.exit(1);
  }

  await ensureDaemon(repoRoot);
  const client = new GitimClient(repoRoot);

  const limit = options.limit ? parseInt(options.limit) : undefined;
  const offset = options.offset ? parseInt(options.offset) : undefined;

  if (limit !== undefined && isNaN(limit)) {
    console.error('Invalid limit: must be a number');
    process.exit(1);
  }
  if (offset !== undefined && isNaN(offset)) {
    console.error('Invalid offset: must be a number');
    process.exit(1);
  }

  const result = await client.search({
    query: query || undefined,
    author: options.author,
    channel: options.channel,
    channel_type: options.type,
    limit,
    offset,
  });

  if (!result.ok) {
    console.error(`Search failed: ${result.error}`);
    process.exit(1);
  }

  const { messages, total } = result.data;
  console.log(`Found ${total} results:\n`);

  for (const msg of messages) {
    const prefix = msg.channel_type === 'dm' ? `[DM:${msg.channel}]` : `[#${msg.channel}]`;
    console.log(`${prefix} @${msg.author} (L${msg.line_number}) ${msg.timestamp}`);
    console.log(`  ${msg.body}`);
    console.log();
  }
}
