import fs from 'node:fs';
import path from 'node:path';

export function initRepo(dir: string = process.cwd()): void {
  const dirs = [
    path.join(dir, '.gitim'),
    path.join(dir, 'users'),
    path.join(dir, 'channels'),
  ];

  for (const d of dirs) {
    fs.mkdirSync(d, { recursive: true });
  }

  const configPath = path.join(dir, '.gitim', 'config.yaml');
  if (!fs.existsSync(configPath)) {
    fs.writeFileSync(configPath, 'version: 1\n');
  }

  const gitignorePath = path.join(dir, '.gitignore');
  const gitignoreContent = fs.existsSync(gitignorePath)
    ? fs.readFileSync(gitignorePath, 'utf-8')
    : '';
  if (!gitignoreContent.includes('.gitim/run/')) {
    fs.appendFileSync(gitignorePath, '\n.gitim/run/\n');
  }

  console.log('GitIM repository initialized.');
}
