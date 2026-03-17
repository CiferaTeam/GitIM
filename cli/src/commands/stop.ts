import { findRepoRoot, isDaemonRunning } from '../daemon.js';
import { GitimClient } from '../client.js';

export async function stopCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('Not in a GitIM repository');
    process.exit(1);
  }

  if (!isDaemonRunning(repoRoot)) {
    console.log('Daemon is not running.');
    return;
  }

  const client = new GitimClient(repoRoot);
  try {
    await client.stop();
    console.log('Daemon stopped.');
  } catch {
    console.log('Daemon stopped.');
  }
}
