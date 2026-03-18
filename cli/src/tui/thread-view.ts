/**
 * 线程链视图组件（在 Overlay 中显示）
 */
import chalk from 'chalk';
import { type Component, truncateToWidth, visibleWidth } from '@mariozechner/pi-tui';
import type { Message } from '../daemon-connection.js';
import { authorColor, colors } from './themes.js';

function formatTime(ts: string): string {
  const m = ts.match(/T(\d{2})(\d{2})/);
  if (!m) return '??:??';
  return `${m[1]}:${m[2]}`;
}

export class ThreadView implements Component {
  messages: Message[] = [];
  rootLine = 0;
  selectedIndex = 0;

  onReply?: (msg: Message) => void;
  onClose?: () => void;

  invalidate(): void {}

  moveUp(): void {
    if (this.selectedIndex > 0) this.selectedIndex--;
  }

  moveDown(): void {
    if (this.selectedIndex < this.messages.length - 1) this.selectedIndex++;
  }

  handleInput(data: string): void {
    // 由 app.ts 处理
  }

  render(width: number): string[] {
    const lines: string[] = [];
    const innerW = width - 2; // 边框

    // 标题
    const title = ` 引用链: L${this.rootLine} `;
    const titleLine = '┌' + chalk.bold(title) + '─'.repeat(Math.max(0, width - 2 - visibleWidth(title))) + '┐';
    lines.push(truncateToWidth(titleLine, width));

    if (this.messages.length === 0) {
      lines.push(truncateToWidth('│' + chalk.dim(' 无消息').padEnd(innerW) + '│', width));
    } else {
      for (let i = 0; i < this.messages.length; i++) {
        const msg = this.messages[i];
        const isLast = i === this.messages.length - 1;
        const isSelected = i === this.selectedIndex;
        const colorFn = authorColor(msg.author);

        // 树形连接符
        const branch = i === 0 ? '┌' : isLast ? '└' : '├';
        const branchStr = colors.threadBranch(branch + '─ ');

        const time = colors.timestamp(formatTime(msg.timestamp));
        const author = colorFn(`@${msg.author}`);
        const bodyPreview = msg.body.slice(0, Math.max(10, innerW - 25));

        let content = `${branchStr}${author} ${time} ${bodyPreview}`;
        if (isSelected) {
          content = colors.highlight(content);
        }

        lines.push(truncateToWidth('│' + truncateToWidth(content, innerW) + '│', width));

        // 非最后元素之间的竖线
        if (!isLast) {
          lines.push(truncateToWidth('│' + colors.threadBranch('│') + ' '.repeat(Math.max(0, innerW - 1)) + '│', width));
        }
      }
    }

    // 底部边框 + 操作提示
    const hint = chalk.dim(' j/k:选择 r:回复 Esc:关闭 ');
    const bottomLine = '└' + hint + '─'.repeat(Math.max(0, width - 2 - visibleWidth(hint))) + '┘';
    lines.push(truncateToWidth(bottomLine, width));

    return lines;
  }
}
