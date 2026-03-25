/**
 * WebUI 命令入口 — 启动浏览器聊天界面
 */
import { findRepoRoot, ensureDaemon } from '../daemon.js';
import { startServer } from '../webui/server.js';

export interface WebuiOptions {
  port: number;
  dev: boolean;
}

export async function webuiCommand(options: WebuiOptions): Promise<void> {
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

  // 启动 bridge server
  try {
    await startServer({ repoRoot, port: options.port, dev: options.dev });
  } catch (e: any) {
    if (e.message?.includes('already in use')) {
      console.error(`错误：端口 ${options.port} 已被占用`);
      console.error(`  → 使用 --port <port> 指定其他端口`);
    } else {
      console.error(`错误：无法启动 WebUI — ${e.message}`);
    }
    process.exit(1);
  }

  console.log(`\nGitIM WebUI: http://localhost:${options.port}\n`);

  // HTTP server 保持进程运行，Ctrl+C 干净退出
  process.on('SIGINT', () => {
    console.log('\n正在关闭 WebUI...');
    process.exit(0);
  });
}
