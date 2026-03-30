/**
 * TUI 主题定义
 */
import chalk from 'chalk';
import type { EditorTheme, SelectListTheme } from '@mariozechner/pi-tui';

export const selectListTheme: SelectListTheme = {
  selectedPrefix: (text: string) => chalk.cyan(text),
  selectedText: (text: string) => chalk.cyan.bold(text),
  description: (text: string) => chalk.dim(text),
  scrollInfo: (text: string) => chalk.dim(text),
  noMatch: (text: string) => chalk.dim(text),
};

export const editorTheme: EditorTheme = {
  borderColor: (str: string) => chalk.gray(str),
  selectList: selectListTheme,
};

// 颜色方案
export const colors = {
  // 消息作者颜色池
  authorColors: [
    chalk.cyan,
    chalk.green,
    chalk.yellow,
    chalk.magenta,
    chalk.blue,
    chalk.red,
  ],
  timestamp: chalk.dim,
  highlight: chalk.bgCyan.black,
  reply: chalk.dim,
  separator: chalk.gray,
  channelActive: chalk.bgCyan.black,
  channelNormal: chalk.white,
  unread: chalk.yellow,
  statusBar: chalk.bgGray.white,
  modeNormal: chalk.green,
  modeBrowse: chalk.yellow,
  modeChain: chalk.magenta,
  threadBranch: chalk.gray,
};

/**
 * 根据作者名分配固定颜色
 */
export function authorColor(author: string): (text: string) => string {
  let hash = 0;
  for (let i = 0; i < author.length; i++) {
    hash = ((hash << 5) - hash + author.charCodeAt(i)) | 0;
  }
  return colors.authorColors[Math.abs(hash) % colors.authorColors.length];
}
