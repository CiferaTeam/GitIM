#!/usr/bin/env node
import { Command } from 'commander';
import { statusCommand } from './commands/status.js';
import { sendCommand } from './commands/send.js';
import { readCommand } from './commands/read.js';
import { channelsCommand } from './commands/channels.js';
import { usersCommand } from './commands/users.js';
import { dmSendCommand, dmReadCommand, dmListCommand } from './commands/dm.js';
import { onboardCommand } from './commands/onboard.js';
import { stopCommand } from './commands/stop.js';

const program = new Command();

program
  .name('gitim')
  .description('GitIM CLI — AI-native IM over Git')
  .version('0.1.0');

program
  .command('onboard [repo_name] [org]')
  .description('加入或创建 GitIM 仓库')
  .option('-g, --git-server <type>', 'Git 服务类型: git | github | gitea | gitlab', 'github')
  .option('-t, --token <token>', 'GitHub/Gitea/GitLab 认证 token')
  .option('--handler <handler>', 'git 本地模式必填：本地 handler')
  .option('--display-name <name>', 'git 本地模式必填：显示名称')
  .option('-u, --url <url>', 'Gitea/GitLab 服务地址')
  .option('--refresh', '重新推断身份')
  .action(async (repoName, org, options) => {
    await onboardCommand(repoName, org, options);
  });

program
  .command('status')
  .description('Show daemon status')
  .action(() => statusCommand());

program
  .command('send <channel> <body>')
  .description('Send a message to a channel')
  .option('-a, --author <handler>', '作者 handler（可选，默认使用 onboard 身份）')
  .option('-r, --reply-to <line>', 'Reply to line number')
  .action((channel, body, options) => sendCommand(channel, body, options));

program
  .command('read <channel>')
  .description('Read messages from a channel')
  .option('-l, --limit <n>', 'Limit number of messages')
  .option('-s, --since <line>', 'Show messages since line number')
  .action((channel, options) => readCommand(channel, options));

program
  .command('channels')
  .description('List channels')
  .action(() => channelsCommand());

program
  .command('users')
  .description('List users')
  .action(() => usersCommand());

program
  .command('stop')
  .description('停止当前仓库的 daemon')
  .action(async () => {
    await stopCommand();
  });

const dm = program.command('dm').description('Direct messages');

dm.command('send <handler> <body>')
  .description('Send a DM')
  .option('-a, --author <handler>', '作者 handler（可选，默认使用 onboard 身份）')
  .option('-r, --reply-to <line>', 'Reply to line number')
  .action((handler, body, options) => dmSendCommand(handler, body, options));

dm.command('read <handler>')
  .description('Read DM conversation')
  .option('-a, --author <handler>', '作者 handler（可选，默认使用 onboard 身份）')
  .option('-l, --limit <n>', 'Limit messages')
  .option('-s, --since <line>', 'Since line number')
  .action((handler, options) => dmReadCommand(handler, options));

dm.command('list')
  .description('List DM conversations')
  .action(() => dmListCommand());

program.parse();
