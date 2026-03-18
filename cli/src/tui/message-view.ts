/**
 * 消息视图组件
 */
import chalk from 'chalk';
import { type Component, truncateToWidth, visibleWidth, wrapTextWithAnsi } from '@mariozechner/pi-tui';
import type { Message } from '../daemon-connection.js';
import { authorColor, colors } from './themes.js';

/**
 * 格式化时间戳 20260317T120000Z → 12:00
 */
function formatTime(ts: string): string {
  const m = ts.match(/T(\d{2})(\d{2})/);
  if (!m) return '??:??';
  return `${m[1]}:${m[2]}`;
}

export class MessageView implements Component {
  messages: Message[] = [];
  /** 浏览模式中选中的索引（messages 数组中的索引） */
  selectedIndex = -1;
  /** 视口偏移（从底部算起的行数偏移） */
  private scrollOffset = 0;
  /** 是否处于浏览模式 */
  browsing = false;

  /** 回复选中消息的回调 */
  onReply?: (msg: Message) => void;
  /** 展开线程链的回调 */
  onThread?: (msg: Message) => void;

  invalidate(): void {}

  /**
   * 查找消息（用于显示回复摘要）
   */
  private findMessage(lineNumber: number): Message | undefined {
    return this.messages.find(m => m.line_number === lineNumber);
  }

  /**
   * 进入浏览模式
   */
  enterBrowse(): void {
    this.browsing = true;
    if (this.messages.length > 0) {
      this.selectedIndex = this.messages.length - 1;
    }
    this.scrollOffset = 0;
  }

  /**
   * 退出浏览模式
   */
  exitBrowse(): void {
    this.browsing = false;
    this.selectedIndex = -1;
    this.scrollOffset = 0;
  }

  moveUp(): void {
    if (this.selectedIndex > 0) this.selectedIndex--;
  }

  moveDown(): void {
    if (this.selectedIndex < this.messages.length - 1) this.selectedIndex++;
  }

  jumpToBottom(): void {
    if (this.messages.length > 0) {
      this.selectedIndex = this.messages.length - 1;
      this.scrollOffset = 0;
    }
  }

  handleInput(data: string): void {
    // 由 app.ts 处理
  }

  render(width: number): string[] {
    if (this.messages.length === 0) {
      return [chalk.dim(' 暂无消息')];
    }

    // 渲染所有消息为行
    const allLines: string[] = [];
    for (let i = 0; i < this.messages.length; i++) {
      const msg = this.messages[i];
      const isSelected = this.browsing && i === this.selectedIndex;
      const rendered = this.renderMessage(msg, width, isSelected);
      allLines.push(...rendered);
    }

    return allLines;
  }

  /**
   * 渲染带视口的消息（需要知道可用高度）
   */
  renderWithHeight(width: number, height: number): string[] {
    if (this.messages.length === 0) {
      const lines = Array(height).fill('');
      lines[Math.floor(height / 2)] = chalk.dim('  暂无消息');
      return lines;
    }

    // 渲染所有消息
    const blocks: { lines: string[]; msgIndex: number }[] = [];
    for (let i = 0; i < this.messages.length; i++) {
      const msg = this.messages[i];
      const isSelected = this.browsing && i === this.selectedIndex;
      const rendered = this.renderMessage(msg, width, isSelected);
      blocks.push({ lines: rendered, msgIndex: i });
    }

    const allLines = blocks.flatMap(b => b.lines);

    // 确保选中的消息在视口中可见
    if (this.browsing && this.selectedIndex >= 0) {
      // 计算选中消息在 allLines 中的起始行
      let selectedStart = 0;
      for (let i = 0; i < blocks.length && blocks[i].msgIndex < this.selectedIndex; i++) {
        selectedStart += blocks[i].lines.length;
      }
      const selectedBlock = blocks.find(b => b.msgIndex === this.selectedIndex);
      const selectedEnd = selectedStart + (selectedBlock?.lines.length ?? 1);

      // 调整 scrollOffset 使选中消息可见
      if (selectedEnd > allLines.length - this.scrollOffset) {
        this.scrollOffset = allLines.length - selectedEnd;
      }
      if (selectedStart < allLines.length - this.scrollOffset - height) {
        this.scrollOffset = allLines.length - selectedStart - height;
      }
      if (this.scrollOffset < 0) this.scrollOffset = 0;
    }

    // 从底部截取 height 行
    const endIdx = allLines.length - this.scrollOffset;
    const startIdx = Math.max(0, endIdx - height);
    const visible = allLines.slice(startIdx, endIdx);

    // 如果行数不够，在顶部填空行
    while (visible.length < height) {
      visible.unshift('');
    }

    return visible;
  }

  private renderMessage(msg: Message, width: number, selected: boolean): string[] {
    const lines: string[] = [];
    const colorFn = authorColor(msg.author);
    const time = colors.timestamp(formatTime(msg.timestamp));
    const author = colorFn(`@${msg.author}`);

    // 回复提示行
    if (msg.point_to > 0) {
      const parent = this.findMessage(msg.point_to);
      const parentSummary = parent
        ? `@${parent.author}: ${parent.body.slice(0, 30)}${parent.body.length > 30 ? '...' : ''}`
        : `L${msg.point_to}`;
      const replyLine = colors.reply(`  ↩ ${parentSummary}`);
      lines.push(truncateToWidth(replyLine, width));
    }

    // 主消息行
    const header = `${author} ${time} `;
    const headerW = visibleWidth(header);
    const bodyW = Math.max(10, width - headerW);

    // 消息正文可能需要换行
    const bodyLines = wrapTextWithAnsi(msg.body, bodyW);
    if (bodyLines.length === 0) {
      lines.push(truncateToWidth(header, width));
    } else {
      lines.push(truncateToWidth(header + bodyLines[0], width));
      for (let i = 1; i < bodyLines.length; i++) {
        lines.push(truncateToWidth(' '.repeat(headerW) + bodyLines[i], width));
      }
    }

    // 选中高亮
    if (selected) {
      return lines.map(l => colors.highlight(l));
    }

    return lines;
  }
}
