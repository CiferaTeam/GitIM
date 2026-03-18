/**
 * 状态栏组件
 */
import chalk from 'chalk';
import { type Component, truncateToWidth, visibleWidth } from '@mariozechner/pi-tui';
import { colors } from './themes.js';

export type AppMode = 'normal' | 'browse' | 'chain';

export class StatusBar implements Component {
  user = '';
  connected = false;
  mode: AppMode = 'normal';
  channel = '';
  replyTarget: number | null = null;

  invalidate(): void {}

  render(width: number): string[] {
    const modeLabel = this.mode === 'normal' ? colors.modeNormal(' 普通 ')
      : this.mode === 'browse' ? colors.modeBrowse(' 浏览 ')
      : colors.modeChain(' 线程 ');

    const connLabel = this.connected ? chalk.green('●') : chalk.red('●');

    let left = ` ${connLabel} @${this.user} | #${this.channel} ${modeLabel}`;
    if (this.replyTarget) {
      left += chalk.dim(` 回复 L${this.replyTarget}`);
    }

    const hints = this.mode === 'normal'
      ? 'Ctrl+B:浏览 | Alt+↑↓:切频道 | Ctrl+Q:退出'
      : this.mode === 'browse'
      ? 'j/k:选择 | r:回复 | Enter:线程 | Esc:返回 | g:跳底'
      : 'j/k:选择 | r:回复 | Esc:关闭';

    const right = ` ${hints} `;
    const leftW = visibleWidth(left);
    const rightW = visibleWidth(right);
    const pad = Math.max(0, width - leftW - rightW);

    const line = chalk.bgGray.white(
      truncateToWidth(left + ' '.repeat(pad) + right, width)
    );
    return [line];
  }
}
