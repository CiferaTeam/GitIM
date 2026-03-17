import { execSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { ensureDaemon } from '../daemon.js';
import { GitimClient } from '../client.js';

interface InferredIdentity {
  handler: string;
  displayName: string;
  endpoint: string;
}

function inferIdentity(endpoint: string, endpointUrl: string): InferredIdentity {
  if (endpoint === 'github') {
    try {
      const result = execSync('gh api /user', { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] });
      const user = JSON.parse(result);
      return {
        handler: user.login.toLowerCase(),
        displayName: user.name || user.login,
        endpoint: 'github',
      };
    } catch {
      console.error('Error: GitHub 认证不可用');
      console.error('  → 请运行 `gh auth login` 配置认证');
      process.exit(1);
    }
  } else if (endpoint === 'gitea') {
    const token = process.env.GITEA_TOKEN;
    if (!token) {
      console.error('Error: GITEA_TOKEN 环境变量未设置');
      console.error('  → 请设置 GITEA_TOKEN 环境变量');
      process.exit(1);
    }
    try {
      const result = execSync(
        `curl -sf -H "Authorization: token ${token}" ${endpointUrl}/api/v1/user`,
        { encoding: 'utf-8', stdio: ['pipe', 'pipe', 'pipe'] }
      );
      const user = JSON.parse(result);
      return {
        handler: user.login.toLowerCase(),
        displayName: user.full_name || user.login,
        endpoint: 'gitea',
      };
    } catch {
      console.error('Error: Gitea 认证失败');
      console.error(`  → 请确认 GITEA_TOKEN 和服务地址 ${endpointUrl} 正确`);
      process.exit(1);
    }
  }
  console.error(`Error: 不支持的 endpoint: ${endpoint}`);
  process.exit(1);
}

function initGitimRepo(
  repoDir: string,
  identity: InferredIdentity,
  endpoint: string,
  endpointUrl: string,
): void {
  // Create directory structure
  fs.mkdirSync(path.join(repoDir, '.gitim'), { recursive: true });
  fs.mkdirSync(path.join(repoDir, 'users'), { recursive: true });
  fs.mkdirSync(path.join(repoDir, 'channels'), { recursive: true });

  // Write config.yaml
  const configContent = [
    'version: 1',
    `endpoint: ${endpoint}`,
    `endpoint_url: "${endpointUrl}"`,
    '',
  ].join('\n');
  fs.writeFileSync(path.join(repoDir, '.gitim', 'config.yaml'), configContent);

  // Update .gitignore
  const gitignorePath = path.join(repoDir, '.gitignore');
  const existing = fs.existsSync(gitignorePath) ? fs.readFileSync(gitignorePath, 'utf-8') : '';
  const additions: string[] = [];
  if (!existing.includes('.gitim/run/')) additions.push('.gitim/run/');
  if (!existing.includes('.gitim/me.json')) additions.push('.gitim/me.json');
  if (additions.length > 0) {
    fs.appendFileSync(gitignorePath, '\n' + additions.join('\n') + '\n');
  }

  // Write me.json
  writeMeJson(repoDir, identity, endpoint);

  // Create user meta
  const userMeta = JSON.stringify({
    display_name: identity.displayName,
    role: 'member',
    introduction: 'GitIM user',
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, 'users', `${identity.handler}.meta.json`), userMeta);

  // Create default general channel
  const now = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
  const channelMeta = JSON.stringify({
    display_name: 'General',
    created_by: identity.handler,
    created_at: now,
    introduction: '默认频道',
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, 'channels', 'general.meta.json'), channelMeta);
  fs.writeFileSync(path.join(repoDir, 'channels', 'general.thread'), '');

  // Git commit + push
  execSync('git add -A', { cwd: repoDir, stdio: 'ignore' });
  execSync(`git commit -m "feat: initialize GitIM repo by @${identity.handler}"`, { cwd: repoDir, stdio: 'ignore' });
  try {
    execSync('git push -u origin HEAD', { cwd: repoDir, stdio: 'ignore' });
  } catch {
    // Push may fail if no remote, that's ok for local testing
  }
}

function writeMeJson(repoDir: string, identity: InferredIdentity, endpoint: string): void {
  const now = new Date().toISOString().replace(/[-:]/g, '').replace(/\.\d{3}/, '');
  const meJson = JSON.stringify({
    handler: identity.handler,
    endpoint,
    inferred_from: endpoint === 'github' ? 'gh_api' : 'gitea_api',
    inferred_at: now,
  }, null, 2);
  fs.writeFileSync(path.join(repoDir, '.gitim', 'me.json'), meJson);
}

export async function onboardCommand(
  repoName: string | undefined,
  org: string | undefined,
  options: { endpoint: string; url: string; refresh: boolean },
): Promise<void> {
  const endpoint = options.endpoint || 'github';
  const endpointUrl = options.url || '';

  // --refresh mode: re-infer identity in current repo
  if (options.refresh) {
    const cwd = process.cwd();
    if (!fs.existsSync(path.join(cwd, '.gitim', 'config.yaml'))) {
      console.error('不在 GitIM 仓库中，无法 --refresh');
      process.exit(1);
    }
    const identity = inferIdentity(endpoint, endpointUrl);
    writeMeJson(cwd, identity, endpoint);
    console.log(`身份已刷新：@${identity.handler}`);
    return;
  }

  if (!repoName) {
    console.error('请指定仓库名称: gitim onboard <repo_name> [org]');
    process.exit(1);
  }

  // 1. Infer identity
  const identity = inferIdentity(endpoint, endpointUrl);
  console.log(`身份推断：@${identity.handler}`);

  // 2. Validate git is available
  try {
    execSync('git --version', { stdio: 'ignore' });
  } catch {
    console.error('Error: Git 命令不可用');
    console.error('  → 请安装 Git: https://git-scm.com/');
    process.exit(1);
  }

  // 3. Determine repo URL and try clone
  const owner = org || identity.handler;
  let repoUrl: string;
  if (endpoint === 'github') {
    repoUrl = `https://github.com/${owner}/${repoName}.git`;
  } else {
    repoUrl = `${endpointUrl}/${owner}/${repoName}.git`;
  }

  const targetDir = path.resolve(repoName);
  let cloneSucceeded = false;

  try {
    execSync(`git clone ${repoUrl} ${targetDir}`, { stdio: 'ignore' });
    cloneSucceeded = true;
  } catch {
    cloneSucceeded = false;
  }

  if (cloneSucceeded) {
    // Check if it's already a GitIM repo
    const isGitim = fs.existsSync(path.join(targetDir, '.gitim', 'config.yaml'));

    if (isGitim) {
      // 4a: Load flow
      writeMeJson(targetDir, identity, endpoint);
      await ensureDaemon(targetDir);
      const client = new GitimClient(targetDir);
      await client.registerUser(identity.handler, identity.displayName);
      console.log(`已加入 ${repoName}，身份：@${identity.handler}`);
    } else {
      // 4b: Init flow
      initGitimRepo(targetDir, identity, endpoint, endpointUrl);
      await ensureDaemon(targetDir);
      console.log(`已初始化 ${repoName}，身份：@${identity.handler}`);
    }
  } else {
    // 4c: Create flow — repo doesn't exist
    if (endpoint === 'github') {
      const ghRepo = org ? `${org}/${repoName}` : repoName;
      try {
        execSync(`gh repo create ${ghRepo} --private --clone`, {
          cwd: path.dirname(targetDir),
          stdio: 'ignore',
        });
      } catch {
        console.error(`Error: 无法创建仓库 ${ghRepo}`);
        console.error('  → 请确认 Token 有仓库创建权限');
        process.exit(1);
      }
    } else {
      // Gitea create repo via API
      const token = process.env.GITEA_TOKEN!;
      const createUrl = org
        ? `${endpointUrl}/api/v1/orgs/${org}/repos`
        : `${endpointUrl}/api/v1/user/repos`;
      try {
        execSync(
          `curl -sf -X POST -H "Authorization: token ${token}" -H "Content-Type: application/json" -d '{"name":"${repoName}","private":true}' ${createUrl}`,
          { stdio: 'ignore' },
        );
        execSync(`git clone ${repoUrl} ${targetDir}`, { stdio: 'ignore' });
      } catch {
        console.error(`Error: 无法创建 Gitea 仓库 ${repoName}`);
        process.exit(1);
      }
    }
    initGitimRepo(targetDir, identity, endpoint, endpointUrl);
    await ensureDaemon(targetDir);
    console.log(`已创建并初始化 ${repoName}，身份：@${identity.handler}`);
  }
}
