/**
 * TUI 主应用 — 组装所有组件并管理交互
 */
import { TUI, ProcessTerminal, Editor, matchesKey, Key } from '@mariozechner/pi-tui';
import type { OverlayHandle } from '@mariozechner/pi-tui';
import { DaemonConnection, type Message, type PushEvent } from './daemon-connection.js';
import { ChannelSidebar, SIDEBAR_WIDTH } from './channel-sidebar.js';
import { MessageView } from './message-view.js';
import { SplitLayout } from './split-layout.js';
import { StatusBar, type AppMode } from './status-bar.js';
import { ThreadView } from './thread-view.js';
import { MentionPopup } from './mention-popup.js';
import { editorTheme } from './themes.js';

export interface TuiAppOptions {
  repoRoot: string;
  user: string;
}

export class TuiApp {
  private tui: TUI;
  private terminal: ProcessTerminal;
  private conn: DaemonConnection;
  private user: string;
  private repoRoot: string;

  // 组件
  private sidebar: ChannelSidebar;
  private messageView: MessageView;
  private editor: Editor;
  private statusBar: StatusBar;
  private splitLayout: SplitLayout;
  private threadView: ThreadView;
  private mentionPopup: MentionPopup;

  // 状态
  private mode: AppMode = 'normal';
  private users: string[] = [];
  private threadOverlay: OverlayHandle | null = null;
  private mentionOverlay: OverlayHandle | null = null;
  private mentionFilter = '';
  private loadMessagesInFlight = false;
  private loadMessagesPending = false;

  constructor(options: TuiAppOptions) {
    this.repoRoot = options.repoRoot;
    this.user = options.user;

    this.terminal = new ProcessTerminal();
    this.tui = new TUI(this.terminal);
    this.conn = new DaemonConnection(this.repoRoot);

    // 初始化组件
    this.sidebar = new ChannelSidebar();
    this.messageView = new MessageView();
    this.editor = new Editor(this.tui, editorTheme);
    this.statusBar = new StatusBar();
    this.threadView = new ThreadView();
    this.mentionPopup = new MentionPopup();

    this.splitLayout = new SplitLayout(this.sidebar, this.messageView, SIDEBAR_WIDTH);

    // 配置状态栏
    this.statusBar.user = this.user;
    this.statusBar.mode = 'normal';

    // 配置 Editor
    this.editor.onSubmit = (text: string) => {
      this.sendMessage(text);
    };

    // 配置线程视图回调
    this.threadView.onReply = (msg: Message) => {
      this.statusBar.replyTarget = msg.line_number;
      this.closeThread();
      this.setMode('normal');
      this.tui.requestRender();
    };

    this.threadView.onClose = () => {
      this.closeThread();
    };

    // 配置提及弹窗回调
    this.mentionPopup.onSelect = (user: string) => {
      this.editor.insertTextAtCursor(user + ' ');
      this.closeMention();
    };

    this.mentionPopup.onCancel = () => {
      this.closeMention();
    };

    // 推送事件处理
    this.conn.onEvent = (event: PushEvent) => {
      if (event.event === 'thread_changed' && event.channel === this.sidebar.currentChannel) {
        this.loadMessages();
      } else if (event.event === 'thread_changed' && event.channel !== this.sidebar.currentChannel) {
        // 更新未读计数
        const count = this.sidebar.unreadCounts.get(event.channel) ?? 0;
        this.sidebar.unreadCounts.set(event.channel, count + 1);
        this.tui.requestRender();
      }
    };

    // 全局输入监听
    this.tui.addInputListener((data: string) => {
      return this.handleGlobalInput(data);
    });
  }

  async start(): Promise<void> {
    // 连接 daemon
    try {
      await this.conn.connect(true);
      this.statusBar.connected = true;
    } catch (e: any) {
      this.statusBar.connected = false;
    }

    // 加载频道和用户
    await this.loadChannels();
    await this.loadUsers();

    // 加载第一个频道的消息
    if (this.sidebar.channels.length > 0) {
      this.statusBar.channel = this.sidebar.currentChannel;
      await this.loadMessages();
    }

    // 组装布局：SplitLayout + Editor + StatusBar
    this.tui.addChild(this.splitLayout);
    this.tui.addChild(this.editor);
    this.tui.addChild(this.statusBar);

    // 设置焦点到 Editor
    this.tui.setFocus(this.editor);

    // 计算布局高度
    this.updateLayoutHeight();

    // 启动 TUI
    this.tui.start();
    this.tui.requestRender();

    // 监听终端大小变化
    process.stdout.on('resize', () => {
      this.updateLayoutHeight();
      this.tui.requestRender();
    });
  }

  private updateLayoutHeight(): void {
    const totalH = this.terminal.rows;
    // 状态栏 1 行，Editor 约 3 行（边框+内容）
    this.splitLayout.height = Math.max(1, totalH - 4);
  }

  private handleGlobalInput(data: string): { consume?: boolean; data?: string } | undefined {
    // Ctrl+Q 退出
    if (matchesKey(data, Key.ctrl('q'))) {
      this.shutdown();
      return { consume: true };
    }

    // 提及弹窗活动时
    if (this.mentionOverlay) {
      // Esc/Enter/↑↓ 由弹窗处理
      if (matchesKey(data, Key.escape) || matchesKey(data, Key.enter) ||
          matchesKey(data, Key.up) || matchesKey(data, Key.down)) {
        this.mentionPopup.handleInput(data);
        this.tui.requestRender();
        return { consume: true };
      }
      // Backspace 更新过滤词
      if (matchesKey(data, Key.backspace)) {
        if (this.mentionFilter.length > 0) {
          this.mentionFilter = this.mentionFilter.slice(0, -1);
          this.mentionPopup.setFilter(this.mentionFilter);
        } else {
          this.closeMention();
        }
        this.tui.requestRender();
        // 不消费，让 editor 也处理 backspace
        return undefined;
      }
      // 普通字符输入 → 更新过滤词，同时让字符进入 editor
      if (data.length === 1 && data >= ' ') {
        if (data === ' ') {
          // 空格关闭弹窗
          this.closeMention();
        } else {
          this.mentionFilter += data;
          this.mentionPopup.setFilter(this.mentionFilter);
          this.tui.requestRender();
        }
        return undefined; // 让 editor 也收到字符
      }
      // 其他键交给弹窗
      this.mentionPopup.handleInput(data);
      this.tui.requestRender();
      return { consume: true };
    }

    // 线程模式
    if (this.mode === 'chain') {
      return this.handleChainInput(data);
    }

    // 浏览模式
    if (this.mode === 'browse') {
      return this.handleBrowseInput(data);
    }

    // 普通模式
    return this.handleNormalInput(data);
  }

  private handleNormalInput(data: string): { consume?: boolean; data?: string } | undefined {
    // Ctrl+B 进入浏览模式
    if (matchesKey(data, Key.ctrl('b'))) {
      this.setMode('browse');
      return { consume: true };
    }

    // Alt+↑/↓ 切换频道
    if (matchesKey(data, Key.alt('up'))) {
      this.sidebar.moveUp();
      this.onChannelChanged();
      return { consume: true };
    }
    if (matchesKey(data, Key.alt('down'))) {
      this.sidebar.moveDown();
      this.onChannelChanged();
      return { consume: true };
    }

    // Alt+1~9 跳转频道
    const digits = ['1', '2', '3', '4', '5', '6', '7', '8', '9'] as const;
    for (let i = 0; i < digits.length; i++) {
      if (matchesKey(data, Key.alt(digits[i]))) {
        this.sidebar.jumpTo(i);
        this.onChannelChanged();
        return { consume: true };
      }
    }

    // @ 触发提及弹窗（在 editor 有焦点时）
    if (data === '@') {
      this.showMention();
      // 不消费，让 @ 字符进入 editor
      return undefined;
    }

    // 不消费，让 editor 处理
    return undefined;
  }

  private handleBrowseInput(data: string): { consume?: boolean; data?: string } | undefined {
    if (matchesKey(data, Key.escape)) {
      this.setMode('normal');
      return { consume: true };
    }
    if (data === 'j' || matchesKey(data, Key.down)) {
      this.messageView.moveDown();
      this.tui.requestRender();
      return { consume: true };
    }
    if (data === 'k' || matchesKey(data, Key.up)) {
      this.messageView.moveUp();
      this.tui.requestRender();
      return { consume: true };
    }
    if (data === 'G') {
      this.messageView.jumpToBottom();
      this.tui.requestRender();
      return { consume: true };
    }
    if (data === 'r') {
      const msg = this.messageView.messages[this.messageView.selectedIndex];
      if (msg) {
        this.statusBar.replyTarget = msg.line_number;
        this.setMode('normal');
      }
      return { consume: true };
    }
    if (matchesKey(data, Key.enter)) {
      const msg = this.messageView.messages[this.messageView.selectedIndex];
      if (msg) {
        this.showThread(msg);
      }
      return { consume: true };
    }

    // Alt+↑/↓ 切换频道（在浏览模式也生效）
    if (matchesKey(data, Key.alt('up'))) {
      this.sidebar.moveUp();
      this.onChannelChanged();
      return { consume: true };
    }
    if (matchesKey(data, Key.alt('down'))) {
      this.sidebar.moveDown();
      this.onChannelChanged();
      return { consume: true };
    }

    return { consume: true }; // 浏览模式消费所有输入
  }

  private handleChainInput(data: string): { consume?: boolean; data?: string } | undefined {
    if (matchesKey(data, Key.escape)) {
      this.closeThread();
      return { consume: true };
    }
    if (data === 'j' || matchesKey(data, Key.down)) {
      this.threadView.moveDown();
      this.tui.requestRender();
      return { consume: true };
    }
    if (data === 'k' || matchesKey(data, Key.up)) {
      this.threadView.moveUp();
      this.tui.requestRender();
      return { consume: true };
    }
    if (data === 'r') {
      const msg = this.threadView.messages[this.threadView.selectedIndex];
      if (msg) {
        this.statusBar.replyTarget = msg.line_number;
        this.closeThread();
        this.setMode('normal');
      }
      return { consume: true };
    }
    return { consume: true };
  }

  private setMode(mode: AppMode): void {
    this.mode = mode;
    this.statusBar.mode = mode;

    if (mode === 'normal') {
      this.messageView.exitBrowse();
      this.tui.setFocus(this.editor);
    } else if (mode === 'browse') {
      this.messageView.enterBrowse();
      this.tui.setFocus(null);
    } else if (mode === 'chain') {
      this.tui.setFocus(null);
    }

    this.tui.requestRender();
  }

  private async sendMessage(text: string): Promise<void> {
    this.statusBar.error = null;
    const trimmed = text.trim();
    if (!trimmed) return;

    const channel = this.sidebar.currentChannel;
    if (!channel) return;

    const params: Record<string, any> = {
      channel,
      body: trimmed,
      author: this.user,
    };

    if (this.statusBar.replyTarget) {
      params.reply_to = this.statusBar.replyTarget;
      this.statusBar.replyTarget = null;
    }

    try {
      const res = await this.conn.request('send', params);
      if (res.ok) {
        this.editor.setText('');
        await this.loadMessages();
      } else {
        this.statusBar.error = res.error ?? '发送失败';
        this.tui.requestRender();
      }
    } catch (e: any) {
      this.statusBar.error = e.message ?? '发送失败';
      this.tui.requestRender();
    }
  }

  private async loadChannels(): Promise<void> {
    try {
      const res = await this.conn.request('channels');
      if (res.ok && res.data?.channels) {
        this.sidebar.channels = res.data.channels;
      }
    } catch {
      // 忽略
    }
  }

  private async loadUsers(): Promise<void> {
    try {
      const res = await this.conn.request('users');
      if (res.ok && res.data?.users) {
        this.users = res.data.users;
        this.mentionPopup.allUsers = this.users;
      }
    } catch {
      // 忽略
    }
  }

  private async loadMessages(): Promise<void> {
    const channel = this.sidebar.currentChannel;
    if (!channel) return;

    // 防止并发请求导致 FIFO 响应错配
    if (this.loadMessagesInFlight) {
      this.loadMessagesPending = true;
      return;
    }

    this.loadMessagesInFlight = true;
    try {
      const res = await this.conn.request('read', { channel, limit: 50, since: 0 });
      if (res.ok && res.data?.messages) {
        this.messageView.messages = res.data.messages;
        this.tui.requestRender();
      }
    } catch {
      // 忽略
    } finally {
      this.loadMessagesInFlight = false;
      // 如果有待处理的请求，执行最后一次
      if (this.loadMessagesPending) {
        this.loadMessagesPending = false;
        this.loadMessages();
      }
    }
  }

  private onChannelChanged(): void {
    this.statusBar.channel = this.sidebar.currentChannel;
    this.statusBar.replyTarget = null;
    this.sidebar.unreadCounts.delete(this.sidebar.currentChannel);
    this.messageView.messages = [];
    this.messageView.exitBrowse();
    this.loadMessages();
    this.tui.requestRender();
  }

  private async showThread(msg: Message): Promise<void> {
    try {
      const res = await this.conn.request('thread', {
        channel: this.sidebar.currentChannel,
        line_number: msg.line_number,
      });

      if (res.ok && res.data?.messages) {
        this.threadView.messages = res.data.messages;
        this.threadView.rootLine = msg.line_number;
        this.threadView.selectedIndex = 0;

        this.threadOverlay = this.tui.showOverlay(this.threadView, {
          anchor: 'center',
          width: '70%',
          maxHeight: '60%',
        });

        this.setMode('chain');
      }
    } catch {
      // 忽略
    }
  }

  private closeThread(): void {
    if (this.threadOverlay) {
      this.threadOverlay.hide();
      this.threadOverlay = null;
    }
    if (this.mode === 'chain') {
      this.setMode('browse');
    }
  }

  private showMention(): void {
    this.mentionFilter = '';
    this.mentionPopup.setFilter('');
    this.mentionOverlay = this.tui.showOverlay(this.mentionPopup, {
      anchor: 'bottom-left',
      width: 30,
      maxHeight: 12,
      offsetY: -4,
      offsetX: SIDEBAR_WIDTH + 2,
    });
    this.tui.requestRender();
  }

  private closeMention(): void {
    if (this.mentionOverlay) {
      this.mentionOverlay.hide();
      this.mentionOverlay = null;
    }
    this.tui.requestRender();
  }

  private shutdown(): void {
    this.conn.disconnect();
    this.tui.stop();
    process.exit(0);
  }
}
