import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';

const DAEMON_STARTUP_TIMEOUT_MS = 5000;
const POLL_INTERVAL_MS = 100;

export function findRepoRoot(from: string = process.cwd()): string | null {
  let dir = from;
  while (true) {
    if (fs.existsSync(path.join(dir, '.gitim'))) {
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
  const sockPath = path.join(repoRoot, '.gitim', 'run', 'gitim.sock');

  if (isDaemonRunning(repoRoot)) {
    // Daemon process exists — wait for socket if not ready yet (startup race)
    if (fs.existsSync(sockPath)) return;
    const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (fs.existsSync(sockPath)) return;
      await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
    }
    throw new Error('daemon is running but socket not ready');
  }

  // Clean up stale runtime files before spawning
  cleanStaleFiles(repoRoot);

  const child = spawn('gitim-daemon', [], {
    cwd: repoRoot,
    detached: true,
    stdio: 'ignore',
  });
  child.unref();

  const deadline = Date.now() + DAEMON_STARTUP_TIMEOUT_MS;

  while (Date.now() < deadline) {
    if (fs.existsSync(sockPath)) return;
    await new Promise((r) => setTimeout(r, POLL_INTERVAL_MS));
  }

  throw new Error('daemon failed to start within timeout');
}

function cleanStaleFiles(repoRoot: string): void {
  const runDir = path.join(repoRoot, '.gitim', 'run');
  const files = ['gitim.pid', 'gitim.sock', 'gitim.port', 'gitim.lock'];
  for (const f of files) {
    const p = path.join(runDir, f);
    try { fs.unlinkSync(p); } catch { /* ignore */ }
  }
}
