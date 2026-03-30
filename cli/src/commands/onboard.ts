import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ensureDaemon, isDaemonRunning } from '../daemon.js';
import { GitimClient } from '../client.js';
import { startServer } from '../webui/server.js';

type GitServer = 'git' | 'github' | 'gitea' | 'gitlab';

interface OnboardOptions {
  gitServer: GitServer;
  token?: string;
  handler?: string;
  displayName?: string;
  url?: string;
  refresh?: boolean;
  debugHttp?: boolean;
  withWebui?: boolean;
  webuiPort?: string;
  webuiDev?: boolean;
  admin?: boolean;
}

function buildAuth(gitServer: GitServer, options: OnboardOptions): Record<string, string> {
  if (gitServer === 'git') {
    return {
      handler: options.handler!,
      display_name: options.displayName!,
    };
  }
  const auth: Record<string, string> = { token: options.token! };
  if ((gitServer === 'gitea' || gitServer === 'gitlab') && options.url) {
    auth.url = options.url;
  }
  return auth;
}

function ensureConfigDebugHttp(repoDir: string, enabled: boolean): void {
  const configPath = path.join(repoDir, '.gitim', 'config.yaml');
  if (fs.existsSync(configPath)) {
    let content = fs.readFileSync(configPath, 'utf-8');
    if (content.includes('debug_http:')) {
      content = content.replace(/debug_http:\s*(true|false)/, `debug_http: ${enabled}`);
    } else if (content.includes('daemon:')) {
      content = content.replace(/daemon:/, `daemon:\n  debug_http: ${enabled}`);
    } else {
      content += `\ndaemon:\n  debug_http: ${enabled}\n`;
    }
    fs.writeFileSync(configPath, content);
  } else {
    fs.mkdirSync(path.join(repoDir, '.gitim'), { recursive: true });
    fs.writeFileSync(configPath, `version: 1\ndaemon:\n  debug_http: ${enabled}\n`);
  }
}

function validateParams(gitServer: GitServer, options: OnboardOptions): void {
  if (gitServer === 'git') {
    if (!options.handler) {
      console.error('Error: git 本地模式需要 --handler');
      process.exit(1);
    }
    if (!options.displayName) {
      console.error('Error: git 本地模式需要 --display-name');
      process.exit(1);
    }
  } else {
    if (!options.token) {
      console.error(`Error: ${gitServer} 模式需要 --token`);
      process.exit(1);
    }
    if ((gitServer === 'gitea' || gitServer === 'gitlab') && !options.url) {
      console.error(`Error: ${gitServer} 模式需要 --url（服务地址）`);
      process.exit(1);
    }
  }
}

function cloneOrCreateRepo(
  repoName: string,
  org: string | undefined,
  gitServer: GitServer,
  options: OnboardOptions,
): string {
  const targetDir = path.resolve(repoName);

  // Determine repo URL (not applicable for plain git local mode)
  if (gitServer === 'git') {
    // Local mode: just create directory + git init
    fs.mkdirSync(targetDir, { recursive: true });
    try {
      execFileSync('git', ['init'], { cwd: targetDir, stdio: 'ignore' });
    } catch {
      console.error('Error: git init 失败');
      process.exit(1);
    }
    return targetDir;
  }

  // Try clone first, then create if needed
  let cloneSucceeded = false;

  if (gitServer === 'github') {
    // GitHub: use gh CLI which resolves owner automatically
    const ghTarget = org ? `${org}/${repoName}` : repoName;
    try {
      execFileSync('gh', ['repo', 'clone', ghTarget, targetDir], { stdio: 'ignore' });
      cloneSucceeded = true;
    } catch {
      cloneSucceeded = false;
    }

    if (!cloneSucceeded) {
      try {
        execFileSync('gh', ['repo', 'create', ghTarget, '--private', '--clone'], {
          cwd: path.dirname(targetDir),
          stdio: 'ignore',
        });
      } catch {
        console.error(`Error: 无法创建仓库 ${ghTarget}`);
        console.error('  → 请确认 gh 已认证且 Token 有仓库创建权限');
        process.exit(1);
      }
    }
  } else {
    // Gitea / GitLab: org is required for URL construction
    if (!org) {
      console.error(`Error: ${gitServer} 模式需要指定 org（作为 URL 中的 owner）`);
      console.error('  → 用法: gitim onboard <repo> <org> --git-server gitea --url ...');
      process.exit(1);
    }

    const baseUrl = options.url!;
    const repoUrl = `${baseUrl}/${org}/${repoName}.git`;

    try {
      execFileSync('git', ['clone', repoUrl, targetDir], { stdio: 'ignore' });
      cloneSucceeded = true;
    } catch {
      cloneSucceeded = false;
    }

    if (!cloneSucceeded) {
      if (gitServer === 'gitlab') {
        console.error('Error: GitLab 不支持自动创建仓库，请先在 GitLab 上手动创建');
        console.error(`  → 创建后再运行: gitim onboard ${repoName} ${org} --git-server gitlab --url ${baseUrl} --token ...`);
        process.exit(1);
      }

      // Gitea: create via API then clone
      const token = options.token!;
      const createUrl = `${baseUrl}/api/v1/orgs/${org}/repos`;
      try {
        execFileSync('curl', [
          '-sf', '-X', 'POST',
          '-H', `Authorization: token ${token}`,
          '-H', 'Content-Type: application/json',
          '-d', JSON.stringify({ name: repoName, private: true }),
          createUrl,
        ], { stdio: 'ignore' });
        execFileSync('git', ['clone', repoUrl, targetDir], { stdio: 'ignore' });
      } catch {
        console.error(`Error: 无法创建 Gitea 仓库 ${repoName}`);
        process.exit(1);
      }
    }
  }

  return targetDir;
}

export async function onboardCommand(
  repoName: string | undefined,
  org: string | undefined,
  options: OnboardOptions,
): Promise<void> {
  const gitServer: GitServer = (options.gitServer || 'github') as GitServer;

  // --refresh mode: send Onboard request to running daemon
  if (options.refresh) {
    validateParams(gitServer, options);
    const cwd = process.cwd();
    const gitimDir = path.join(cwd, '.gitim');
    if (!fs.existsSync(gitimDir)) {
      console.error('不在 GitIM 仓库中，无法 --refresh');
      process.exit(1);
    }
    // If --debug-http is set, update config and restart daemon
    if (options.debugHttp) {
      ensureConfigDebugHttp(cwd, true);
      if (isDaemonRunning(cwd)) {
        const oldClient = new GitimClient(cwd);
        await oldClient.stop().catch(() => {});
        // Wait briefly for daemon to exit
        await new Promise((r) => setTimeout(r, 300));
      }
    }
    await ensureDaemon(cwd);
    const client = new GitimClient(cwd);
    const auth = buildAuth(gitServer, options);
    const res = await client.onboard(gitServer, auth, options.admin);
    if (!res.ok) {
      console.error(`身份刷新失败：${res.error}`);
      process.exit(1);
    }
    const adminTag = options.admin ? ' [ADMIN]' : '';
    console.log(`身份已刷新：@${res.data?.handler}${adminTag}`);
    if (options.withWebui) {
      await launchWebui(cwd, options);
    }
    return;
  }

  if (!repoName) {
    console.error('请指定仓库名称: gitim onboard <repo_name> [org]');
    process.exit(1);
  }

  // 1. Validate params
  validateParams(gitServer, options);

  // 2. Clone or create repo
  const repoDir = cloneOrCreateRepo(repoName, org, gitServer, options);

  // 3. Ensure .gitim/ directory exists
  fs.mkdirSync(path.join(repoDir, '.gitim'), { recursive: true });

  // 3.5. Write config with debug_http if requested (before daemon starts)
  if (options.debugHttp) {
    ensureConfigDebugHttp(repoDir, true);
  }

  // 4. Start daemon
  await ensureDaemon(repoDir);

  // 5. Send Onboard request
  const client = new GitimClient(repoDir);
  const auth = buildAuth(gitServer, options);
  const res = await client.onboard(gitServer, auth, options.admin);
  if (!res.ok) {
    console.error(`Onboard 失败：${res.error}`);
    process.exit(1);
  }

  // 6. Report result
  const handler = res.data?.handler ?? '(unknown)';
  const created = res.data?.created ? '（新建）' : '（已加入）';
  const adminTag = options.admin ? ' [ADMIN]' : '';
  console.log(`成功 ${created}：@${handler}${adminTag} @ ${repoName}`);

  // 7. Optional: start WebUI
  if (options.withWebui) {
    await launchWebui(repoDir, options);
  }
}

async function launchWebui(repoDir: string, options: OnboardOptions): Promise<void> {
  const port = parseInt(options.webuiPort || '6868', 10);
  if (isNaN(port) || port < 1 || port > 65535) {
    console.error('错误：--webui-port 必须是 1-65535 之间的数字');
    process.exit(1);
  }
  const dev = options.webuiDev || false;

  try {
    await startServer({ repoRoot: repoDir, port, dev });
  } catch (e: unknown) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.includes('already in use')) {
      console.error(`错误：端口 ${port} 已被占用`);
      console.error('  → 使用 --webui-port <port> 指定其他端口');
    } else {
      console.error(`错误：无法启动 WebUI — ${msg}`);
    }
    process.exit(1);
  }

  // HTTP server 保持进程运行，Ctrl+C 干净退出
  process.on('SIGINT', () => {
    console.log('\n正在关闭 WebUI...');
    process.exit(0);
  });
}
