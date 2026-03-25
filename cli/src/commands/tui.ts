/**
 * TUI 命令入口 — 启动终端聊天界面
 */
import fs from 'node:fs';
import path from 'node:path';
import { execFileSync } from 'node:child_process';
import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { TuiApp } from '../tui/app.js';

/**
 * 获取当前用户身份
 */
function getCurrentUser(repoRoot: string): string {
  // 优先从 .gitim/me.json 读取
  const meFile = path.join(repoRoot, '.gitim', 'me.json');
  if (fs.existsSync(meFile)) {
    try {
      const me = JSON.parse(fs.readFileSync(meFile, 'utf-8'));
      if (me.handler) return me.handler;
    } catch {
      // 忽略解析错误
    }
  }

  // 回退到 git config
  try {
    const name = execFileSync('git', ['config', 'user.name'], {
      encoding: 'utf-8',
      stdio: ['pipe', 'pipe', 'pipe'],
    }).trim().toLowerCase().replace(/\s+/g, '-');
    if (name) return name;
  } catch {
    // 忽略
  }

  return 'unknown';
}

export async function tuiCommand(): Promise<void> {
  const repoRoot = findRepoRoot();
  if (!repoRoot) {
    console.error('错误：不在 GitIM 仓库中');
    console.error('  → 请先运行 `gitim onboard` 加入或创建仓库');
    process.exit(1);
  }

  // 确保 daemon 运行
  try {
    await ensureDaemon(repoRoot);
  } catch (e: any) {
    console.error(`错误：无法启动 daemon — ${e.message}`);
    process.exit(1);
  }

  const user = getCurrentUser(repoRoot);

  const app = new TuiApp({ repoRoot, user });
  await app.start();
}
