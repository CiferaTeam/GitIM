#!/usr/bin/env node
import { Command } from 'commander';

const program = new Command();

program
  .name('gitim')
  .description('GitIM CLI — AI-native IM over Git')
  .version('0.1.0');

program.parse();
