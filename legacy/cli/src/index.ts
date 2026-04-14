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
import { tuiCommand } from './commands/tui.js';
import { webuiCommand } from './commands/webui.js';
import { searchCommand } from './commands/search.js';
import { reindexCommand } from './commands/reindex.js';
import { createChannelCommand } from './commands/create-channel.js';
import { joinChannelCommand } from './commands/join-channel.js';
import { archiveChannelCommand } from './commands/archive-channel.js';
import { archivedChannelsCommand } from './commands/archived-channels.js';
import { boardCreateCommand } from './commands/board-create.js';
import { boardListCommand } from './commands/board-list.js';
import { cardCreateCommand } from './commands/card-create.js';
import { cardListCommand } from './commands/card-list.js';
import { cardReadCommand } from './commands/card-read.js';
import { cardSendCommand } from './commands/card-send.js';
import { cardUpdateCommand } from './commands/card-update.js';

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
  .option('--handler <handler>', 'Handler（git 必填；github 可选，配合 --display-name 替代 --token）')
  .option('--display-name <name>', '显示名称（git 必填；github 可选，配合 --handler 替代 --token）')
  .option('-u, --url <url>', 'Gitea/GitLab 服务地址')
  .option('--refresh', '重新推断身份')
  .option('--debug-http', '开启 HTTP 调试端口')
  .option('--admin', 'admin 模式：poll 返回所有内容（审查视角）')
  .option('--guest', '游客模式：只读，不需要身份认证')
  .option('--with-webui', 'Onboard 完成后启动 WebUI')
  .option('--webui-port <port>', 'WebUI 端口号', '6868')
  .option('--webui-dev', 'WebUI 开发模式（启用 Vite HMR）', false)
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
  .command('create-channel <name>')
  .description('创建新频道')
  .option('--display-name <name>', '频道显示名称')
  .option('--introduction <text>', '频道简介')
  .action(async (name: string, options: { displayName?: string; introduction?: string }) => {
    await createChannelCommand(name, options);
  });

program
  .command('join-channel <channel>')
  .description('加入频道或拉人入群')
  .option('-t, --targets <handlers...>', '要拉入的用户（不指定则自己加入）')
  .action(async (channel: string, options: { targets?: string[] }) => {
    await joinChannelCommand(channel, options);
  });

program
  .command('archive-channel <name>')
  .description('归档频道')
  .action(async (name: string) => {
    await archiveChannelCommand(name);
  });

program
  .command('archived-channels')
  .description('列出已归档频道')
  .action(async () => {
    await archivedChannelsCommand();
  });

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

program
  .command('search [query]')
  .description('搜索消息')
  .option('-a, --author <handler>', '按作者过滤')
  .option('-c, --channel <name>', '限定频道')
  .option('-t, --type <type>', '频道类型: channel | dm')
  .option('-l, --limit <n>', '结果数量限制', '50')
  .option('--offset <n>', '分页偏移', '0')
  .action((query, options) => searchCommand(query, options));

program
  .command('reindex')
  .description('重建搜索索引')
  .action(() => reindexCommand());

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

program
  .command('tui')
  .description('启动 TUI 聊天界面')
  .action(async () => {
    await tuiCommand();
  });

program
  .command('webui')
  .description('启动 WebUI 浏览器聊天界面')
  .option('-p, --port <port>', '服务端口号', '6868')
  .option('--dev', '开发模式（启用 Vite HMR）', false)
  .action(async (options) => {
    await webuiCommand({ port: parseInt(options.port, 10), dev: options.dev });
  });

const board = program.command('board').description('Board management');
board.command('create <name>').description('Create a new board')
  .option('--display-name <name>', 'Display name')
  .option('--statuses <list>', 'Comma-separated statuses')
  .action(async (name: string, options: any) => { await boardCreateCommand(name, options); });
board.command('ls').description('List all boards')
  .action(async () => { await boardListCommand(); });

const card = program.command('card').description('Card management');
card.command('create <board> <title>').description('Create a new card')
  .option('--assignee <handler>', 'Assign to user')
  .option('--status <status>', 'Initial status')
  .action(async (board: string, title: string, options: any) => { await cardCreateCommand(board, title, options); });
card.command('ls <board>').description('List cards in a board')
  .option('--status <status>', 'Filter by status')
  .action(async (board: string, options: any) => { await cardListCommand(board, options); });
card.command('read <board> <card-id>').description('Read card messages')
  .option('--limit <n>', 'Limit messages')
  .option('--since <line>', 'Messages since line number')
  .action(async (board: string, cardId: string, options: any) => { await cardReadCommand(board, cardId, options); });
card.command('send <board> <card-id> <body>').description('Send a message to a card')
  .option('--reply-to <line>', 'Reply to line number')
  .action(async (board: string, cardId: string, body: string, options: any) => { await cardSendCommand(board, cardId, body, options); });
card.command('update <board> <card-id>').description('Update card status or assignee')
  .option('--status <status>', 'New status')
  .option('--assignee <handler>', 'New assignee')
  .action(async (board: string, cardId: string, options: any) => { await cardUpdateCommand(board, cardId, options); });

program.parse();
