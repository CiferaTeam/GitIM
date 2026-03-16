import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const DAEMON_STARTUP_TIMEOUT_MS = 5000;
const POLL_INTERVAL_MS = 100;

export function findRepoRoot(from: string = process.cwd()): string | null {
  let dir = from;
  while (true) {
    if (fs.existsSync(path.join(dir, '.gitim', 'config.yaml'))) {
      return dir;
    }
    const parent = path.dirname(dir);
    if (parent === dir) return null;
    dir = parent;
  }
}

export function isDaemonRunning(repoRoot: string): boolean {
  const pidFile = path.join(repoRoot, '.gitim', 'run', 'gitim.pid');
  if (!fs.existsSync(pidFile)) return false;
  const pid = parseInt(fs.readFileSync(pidFile, 'utf-8').trim(), 10);
  if (isNaN(pid)) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

export async function ensureDaemon(repoRoot: string): Promise<void> {
  if (isDaemonRunning(repoRoot)) return;

  const child = spawn('gitim-daemon', [], {
    cwd: repoRoot,
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  const sockPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');
  const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return;
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  throw new Error('daemon failed to start within timeout');
}
