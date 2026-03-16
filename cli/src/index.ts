#!/usr/bin/env node
import { Command } from 'commander';
import { initRepo } from './commands/init.js';
import { statusCommand } from './commands/status.js';
import { sendCommand } from './commands/send.js';
import { readCommand } from './commands/read.js';
import { channelsCommand } from './commands/channels.js';
import { usersCommand } from './commands/users.js';

const program = new Command();

program
  .name('gitim')
  .description('GitIM CLI — AI-native IM over Git')
  .version('0.1.0');

program
  .command('init')
  .description('Initialize a GitIM repository')
  .action(() => initRepo());

program
  .command('status')
  .description('Show daemon status')
  .action(() => statusCommand());

program
  .command('send <channel> <body>')
  .description('Send a message to a channel')
  .requiredOption('-a, --author <handler>', 'Author handler')
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

program.parse();
