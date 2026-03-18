/**
 * @提及弹窗组件（在 Overlay 中显示）
 */
import chalk from 'chalk';
import { type Component, truncateToWidth } from '@mariozechner/pi-tui';
import { Key, matchesKey } from '@mariozechner/pi-tui';

export class MentionPopup implements Component {
  allUsers: string[] = [];
  private filtered: string[] = [];
  private filter = '';
  selectedIndex = 0;

  onSelect?: (user: string) => void;
  onCancel?: () => void;

  setFilter(text: string): void {
    this.filter = text.toLowerCase();
    this.filtered = this.allUsers.filter(u =>
      u.toLowerCase().includes(this.filter)
    );
    this.selectedIndex = 0;
  }

  invalidate(): void {}

  handleInput(data: string): void {
    if (matchesKey(data, Key.up)) {
      if (this.selectedIndex > 0) this.selectedIndex--;
    } else if (matchesKey(data, Key.down)) {
      if (this.selectedIndex < this.filtered.length - 1) this.selectedIndex++;
    } else if (matchesKey(data, Key.enter)) {
      if (this.filtered[this.selectedIndex]) {
        this.onSelect?.(this.filtered[this.selectedIndex]);
      }
    } else if (matchesKey(data, Key.escape)) {
      this.onCancel?.();
    }
  }

  render(width: number): string[] {
    const lines: string[] = [];
    const items = this.filtered.length > 0 ? this.filtered : this.allUsers;
    const title = chalk.bold(' @提及用户 ');
    lines.push(truncateToWidth(title, width));
    lines.push(truncateToWidth(chalk.gray('─'.repeat(width)), width));

    const maxShow = Math.min(items.length, 8);
    for (let i = 0; i < maxShow; i++) {
      const user = items[i];
      if (i === this.selectedIndex) {
        lines.push(truncateToWidth(chalk.bgCyan.black(` ▸ @${user} `.padEnd(width)), width));
      } else {
        lines.push(truncateToWidth(` @${user}`.padEnd(width), width));
      }
    }

    if (items.length === 0) {
      lines.push(truncateToWidth(chalk.dim(' 无匹配用户'), width));
    }

    return lines;
  }
}
