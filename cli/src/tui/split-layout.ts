/**
 * 分栏布局组件 — 左侧（侧边栏）和右侧（主区域）横向拼接
 */
import { type Component, truncateToWidth, visibleWidth } from '@mariozechner/pi-tui';
import chalk from 'chalk';

export interface RightPanel {
  renderWithHeight(width: number, height: number): string[];
}

export class SplitLayout implements Component {
  left: Component;
  right: RightPanel;
  sidebarWidth: number;
  /** 可用高度（不含状态栏） */
  height = 24;

  constructor(left: Component, right: RightPanel, sidebarWidth: number) {
    this.left = left;
    this.right = right;
    this.sidebarWidth = sidebarWidth;
  }

  invalidate(): void {
    this.left.invalidate();
  }

  render(width: number): string[] {
    const leftW = this.sidebarWidth;
    const sepW = 1; // │
    const rightW = Math.max(1, width - leftW - sepW);

    const leftLines = this.left.render(leftW);
    const rightLines = this.right.renderWithHeight(rightW, this.height);

    const maxLines = Math.max(leftLines.length, rightLines.length, this.height);
    const result: string[] = [];

    for (let i = 0; i < maxLines; i++) {
      const l = leftLines[i] ?? '';
      const r = rightLines[i] ?? '';

      // 左侧填充到固定宽度
      const lPadded = padToWidth(l, leftW);
      const sep = chalk.gray('│');
      const line = lPadded + sep + r;
      result.push(truncateToWidth(line, width));
    }

    return result;
  }
}

/**
 * 将一行补齐到指定可见宽度
 */
function padToWidth(line: string, targetWidth: number): string {
  const w = visibleWidth(line);
  if (w >= targetWidth) return truncateToWidth(line, targetWidth);
  return line + ' '.repeat(targetWidth - w);
}
