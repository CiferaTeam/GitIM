/**
 * 频道侧边栏组件
 */
import chalk from 'chalk';
import { type Component, truncateToWidth } from '@mariozechner/pi-tui';
import { colors } from './themes.js';

export const SIDEBAR_WIDTH = 20;

export class ChannelSidebar implements Component {
  channels: string[] = [];
  currentIndex = 0;
  unreadCounts: Map<string, number> = new Map();

  invalidate(): void {}

  get currentChannel(): string {
    return this.channels[this.currentIndex] ?? '';
  }

  selectChannel(name: string): boolean {
    const idx = this.channels.indexOf(name);
    if (idx >= 0) {
      this.currentIndex = idx;
      return true;
    }
    return false;
  }

  moveUp(): void {
    if (this.currentIndex > 0) this.currentIndex--;
  }

  moveDown(): void {
    if (this.currentIndex < this.channels.length - 1) this.currentIndex++;
  }

  jumpTo(n: number): void {
    if (n >= 0 && n < this.channels.length) this.currentIndex = n;
  }

  render(width: number): string[] {
    const w = Math.min(width, SIDEBAR_WIDTH);
    const lines: string[] = [];

    // 标题
    lines.push(truncateToWidth(chalk.bold(' 频道'), w));
    lines.push(truncateToWidth(chalk.gray('─'.repeat(w)), w));

    for (let i = 0; i < this.channels.length; i++) {
      const ch = this.channels[i];
      const unread = this.unreadCounts.get(ch) ?? 0;
      const prefix = i === this.currentIndex ? '▸' : ' ';
      let label = `${prefix} #${ch}`;
      if (unread > 0) {
        label += ` (${unread})`;
      }

      if (i === this.currentIndex) {
        lines.push(truncateToWidth(colors.channelActive(label.padEnd(w)), w));
      } else if (unread > 0) {
        lines.push(truncateToWidth(colors.unread(label.padEnd(w)), w));
      } else {
        lines.push(truncateToWidth(colors.channelNormal(label.padEnd(w)), w));
      }
    }

    return lines;
  }
}
