import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

type GitServer = 'git' | 'github' | 'gitea' | 'gitlab';

interface OnboardOptions {
  gitServer: GitServer;
  token?: string;
  handler?: string;
  displayName?: string;
  url?: string;
  refresh?: boolean;
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

  const owner = org || (gitServer === 'github' ? '' : '');
  let repoUrl: string;
  if (gitServer === 'github') {
    const ghOwner = org || '';
    repoUrl = ghOwner
      ? `https://github.com/${ghOwner}/${repoName}.git`
      : `https://github.com/${repoName}.git`;
  } else {
    // gitea or gitlab
    const baseUrl = options.url!;
    repoUrl = owner
      ? `${baseUrl}/${owner}/${repoName}.git`
      : `${baseUrl}/${repoName}.git`;
  }

  // Try clone first
  let cloneSucceeded = false;
  try {
    execFileSync('git', ['clone', repoUrl, targetDir], { stdio: 'ignore' });
    cloneSucceeded = true;
  } catch {
    cloneSucceeded = false;
  }

  if (!cloneSucceeded) {
    // Repo doesn't exist yet — create it
    if (gitServer === 'github') {
      const ghRepo = org ? `${org}/${repoName}` : repoName;
      try {
        execFileSync('gh', ['repo', 'create', ghRepo, '--private', '--clone'], {
          cwd: path.dirname(targetDir),
          stdio: 'ignore',
        });
      } catch {
        console.error(`Error: 无法创建仓库 ${ghRepo}`);
        console.error('  → 请确认 Token 有仓库创建权限');
        process.exit(1);
      }
    } else {
      // gitea / gitlab: create via API then clone
      const token = options.token!;
      const baseUrl = options.url!;
      const createUrl = org
        ? `${baseUrl}/api/v1/orgs/${org}/repos`
        : `${baseUrl}/api/v1/user/repos`;
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
        console.error(`Error: 无法创建 ${gitServer} 仓库 ${repoName}`);
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
    await ensureDaemon(cwd);
    const client = new GitimClient(cwd);
    const auth = buildAuth(gitServer, options);
    const res = await client.onboard(gitServer, auth);
    if (!res.ok) {
      console.error(`身份刷新失败：${res.error}`);
      process.exit(1);
    }
    console.log(`身份已刷新：@${res.data?.handler}`);
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

  // 4. Start daemon
  await ensureDaemon(repoDir);

  // 5. Send Onboard request
  const client = new GitimClient(repoDir);
  const auth = buildAuth(gitServer, options);
  const res = await client.onboard(gitServer, auth);
  if (!res.ok) {
    console.error(`Onboard 失败：${res.error}`);
    process.exit(1);
  }

  // 6. Report result
  const handler = res.data?.handler ?? '(unknown)';
  const created = res.data?.created ? '（新建）' : '（已加入）';
  console.log(`成功 ${created}：@${handler} @ ${repoName}`);
}
