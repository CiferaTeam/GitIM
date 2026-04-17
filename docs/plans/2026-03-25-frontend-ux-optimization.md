# 前端 UX 优化：DM 区域 + 消息交互增强

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 补全私信（DM）区域的空状态展示和发起新私信流程，同时为消息列表添加单击回复、双击开线程、点击回复引用跳转等交互能力。

**Architecture:** 纯前端改动，不涉及后端 API 变更。DM 发起利用现有 `/api/send` 的懒创建能力（发送到不存在的 DM 时后端自动创建文件）。消息交互通过 click/dblclick 事件 + 延迟定时器消歧 + `getSelection()` 检测实现，避免单击/双击冲突和文本选中冲突。

**Tech Stack:** React 19, Zustand, TypeScript, CSS

---

## 文件结构

| 操作 | 文件 | 职责 |
|------|------|------|
| 修改 | `webui/src/components/Sidebar.tsx` | DM 区域始终显示 + 发起新私信按钮 + 内联搜索 + 自己置顶 |
| 修改 | `webui/src/components/MessageItem.tsx` | 单击回复 + 双击开线程 + 点击回复引用跳转 + hover 复制按钮 |
| 修改 | `webui/src/components/MessageList.tsx` | 传递 onScrollTo 回调 + 高亮闪烁目标消息 |
| 修改 | `webui/src/components/ThreadPanel.tsx` | 扁平时间序展示所有层级回复（去掉仅一层限制） |
| 修改 | `webui/src/components/InputArea.tsx` | Escape 取消回复 + toggle 回复 |
| 修改 | `webui/src/hooks/useStore.ts` | 新增 highlightLine 状态 |
| 修改 | `webui/src/App.tsx` | 接入新的交互回调（onScrollTo、toggle reply、DM 发起） |
| 修改 | `webui/src/index.css` | 新增样式：DM 搜索框、高亮闪烁动画、可点击回复引用 |

---

## Chunk 1: DM 区域改造

### Task 1: Sidebar DM 区域始终显示 + 发起新私信按钮

**Files:**
- Modify: `webui/src/components/Sidebar.tsx`
- Modify: `webui/src/index.css`

- [ ] **Step 1: 修改 Sidebar — DM 区域始终渲染**

将 `{dmList.length > 0 && (...)}` 条件移除，让「私信」标题和按钮始终显示。增加「发起新私信」按钮和内联搜索框状态。

```tsx
// Sidebar.tsx
import { useState, useMemo } from 'react';
import { useStore } from '../hooks/useStore.js';

interface SidebarProps {
  onChannelSelect: (name: string) => void;
  onStartDm: (targetUser: string) => void;
}

export function Sidebar({ onChannelSelect, onStartDm }: SidebarProps) {
  const channels = useStore((s) => s.channels);
  const currentChannel = useStore((s) => s.currentChannel);
  const currentUser = useStore((s) => s.currentUser);
  const users = useStore((s) => s.users);

  const [dmSearchOpen, setDmSearchOpen] = useState(false);
  const [dmSearchFilter, setDmSearchFilter] = useState('');

  const channelList = channels.filter((c) => c.kind === 'channel');
  const dmList = channels.filter((c) => c.kind === 'dm');

  // 自己和自己的 DM 置顶
  const sortedDmList = useMemo(() => {
    const selfDm = dmList.filter((c) => {
      const parts = c.name.split('--');
      return parts.length === 2 && parts[0] === currentUser && parts[1] === currentUser;
    });
    const otherDm = dmList.filter((c) => {
      const parts = c.name.split('--');
      return !(parts.length === 2 && parts[0] === currentUser && parts[1] === currentUser);
    });
    return [...selfDm, ...otherDm];
  }, [dmList, currentUser]);

  // 搜索过滤用户列表
  const filteredUsers = useMemo(() => {
    const lower = dmSearchFilter.toLowerCase();
    return users.filter((u) => u.toLowerCase().includes(lower));
  }, [users, dmSearchFilter]);

  const handleUserSelect = (user: string) => {
    setDmSearchOpen(false);
    setDmSearchFilter('');
    onStartDm(user);
  };

  return (
    <aside className="sidebar">
      {channelList.length > 0 && (
        <>
          <div className="sidebar-section-title">频道</div>
          {channelList.map((ch) => (
            <div
              key={ch.name}
              className={`sidebar-item ${ch.name === currentChannel ? 'active' : ''}`}
              onClick={() => onChannelSelect(ch.name)}
            >
              <span className="sidebar-item-name"># {ch.name}</span>
              {ch.unreadCount > 0 && (
                <span className="unread-badge">{ch.unreadCount}</span>
              )}
            </div>
          ))}
        </>
      )}

      {/* 私信区域：始终显示 */}
      <div className="sidebar-section-title">私信</div>
      {dmSearchOpen ? (
        <div className="dm-search">
          <input
            className="dm-search-input"
            placeholder="搜索用户..."
            value={dmSearchFilter}
            onChange={(e) => setDmSearchFilter(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Escape') {
                setDmSearchOpen(false);
                setDmSearchFilter('');
              }
            }}
            autoFocus
          />
          <div className="dm-search-list">
            {filteredUsers.map((user) => (
              <div
                key={user}
                className="dm-search-item"
                onClick={() => handleUserSelect(user)}
              >
                @ {user}
              </div>
            ))}
            {filteredUsers.length === 0 && (
              <div className="dm-search-empty">无匹配用户</div>
            )}
          </div>
        </div>
      ) : (
        <div
          className="sidebar-item dm-new-btn"
          onClick={() => setDmSearchOpen(true)}
        >
          + 发起新私信
        </div>
      )}
      {sortedDmList.map((ch) => {
        // 显示对方 handler，而非原始 handler1--handler2 格式
        const parts = ch.name.split('--');
        const otherUser = parts[0] === currentUser ? parts[1] : parts[0];
        const isSelfDm = parts[0] === parts[1];
        const displayName = isSelfDm ? `${otherUser} (我)` : otherUser;
        return (
          <div
            key={ch.name}
            className={`sidebar-item ${ch.name === currentChannel ? 'active' : ''}`}
            onClick={() => onChannelSelect(ch.name)}
          >
            <span className="sidebar-item-name">@ {displayName}</span>
            {ch.unreadCount > 0 && (
              <span className="unread-badge">{ch.unreadCount}</span>
            )}
          </div>
        );
      })}
    </aside>
  );
}
```

- [ ] **Step 2: 添加 DM 搜索相关 CSS**

在 `index.css` 的侧边栏部分末尾添加：

```css
/* 私信搜索 */
.dm-new-btn {
  color: var(--accent) !important;
  font-size: 13px;
}

.dm-search {
  padding: 4px 8px;
}

.dm-search-input {
  width: 100%;
  padding: 6px 8px;
  background: var(--bg-tertiary);
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--text-primary);
  font-size: 13px;
  outline: none;
}

.dm-search-input:focus {
  border-color: var(--accent);
}

.dm-search-list {
  max-height: 200px;
  overflow-y: auto;
  margin-top: 4px;
}

.dm-search-item {
  padding: 6px 8px;
  cursor: pointer;
  font-size: 13px;
  color: var(--text-secondary);
  border-radius: 4px;
}

.dm-search-item:hover {
  background: var(--hover);
  color: var(--text-primary);
}

.dm-search-empty {
  padding: 8px;
  font-size: 12px;
  color: var(--text-secondary);
  text-align: center;
}
```

- [ ] **Step 3: App.tsx 接入 DM 发起逻辑**

在 `App.tsx` 中添加 `handleStartDm` 回调，计算 DM channel 名（两个 handler 按字典序 `--` 连接），若已存在则跳转，不存在则选中该 channel（懒创建——发消息时后端自动建文件）。

```tsx
// App.tsx — 在 handleChannelSelect 下方新增
const handleStartDm = useCallback(
  async (targetUser: string) => {
    // 计算 DM channel 名：两个 handler 按字典序排列，-- 连接
    const sorted = [currentUser, targetUser].sort();
    const dmChannel = `${sorted[0]}--${sorted[1]}`;

    // 检查是否已存在于 channels 列表
    const existing = useStore.getState().channels.find((c) => c.name === dmChannel);
    if (existing) {
      // 已有 DM，直接跳转
      await handleChannelSelect(dmChannel);
    } else {
      // 不存在的 DM — 选中该 channel，清空消息（等用户发第一条消息时后端自动创建）
      selectChannel(dmChannel);
      setMessages([]);
      setThreadRoot(null);
    }
  },
  [currentUser, handleChannelSelect, selectChannel, setMessages, setThreadRoot],
);

// JSX 中传递给 Sidebar
<Sidebar onChannelSelect={handleChannelSelect} onStartDm={handleStartDm} />
```

- [ ] **Step 4: 构建并验证 DM 区域渲染**

```bash
cd webui && npm run build
```

确认无编译错误。

- [ ] **Step 5: 提交**

```bash
git add webui/src/components/Sidebar.tsx webui/src/App.tsx webui/src/index.css
git commit -m "feat(webui): add DM section with new-DM search and self-DM pinning"
```

---

## Chunk 2: 消息交互增强

### Task 2: MessageItem — 单击回复 + 双击开线程 + 点击回复引用跳转 + hover 复制

**Files:**
- Modify: `webui/src/components/MessageItem.tsx`
- Modify: `webui/src/index.css`

- [ ] **Step 1: 重构 MessageItem 交互逻辑**

添加 `onClick`（回复）、`onDoubleClick`（开线程）、回复引用区 `onClick`（跳转）、hover 复制按钮。用 `getSelection()` 避免与文本选中冲突。单击同一条消息 toggle 回复。

```tsx
// MessageItem.tsx
import { useCallback, useRef } from 'react';
import type { Message } from '../lib/types.js';
import { formatTimestamp } from '../lib/types.js';

interface MessageItemProps {
  message: Message;
  replyTarget: Message | null;
  isReplying: boolean; // 当前是否正在回复这条消息
  highlight: boolean;  // 是否高亮闪烁
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
  onScrollTo: (lineNumber: number) => void;
  onCopy: (body: string, lineNumber: number) => void;
  copied: boolean; // 是否刚复制过
}

const STATUS_LABEL: Record<string, string> = {
  sending: '发送中...',
  sent: '已发送 ✓',
  failed: '发送失败 ✗',
};

export function MessageItem({
  message,
  replyTarget,
  isReplying,
  highlight,
  onReply,
  onShowThread,
  onScrollTo,
  onCopy,
  copied,
}: MessageItemProps) {
  const isSending = message._status === 'sending';
  const isSent = message._status === 'sent';
  const isFailed = message._status === 'failed';
  const isPending = isSending || isSent;
  const statusText = message._status ? STATUS_LABEL[message._status] : null;

  // 用定时器消歧单击 vs 双击：单击延迟 250ms，双击时取消单击
  const clickTimerRef = useRef<number | null>(null);

  const handleBodyClick = useCallback(() => {
    const sel = window.getSelection();
    if (sel && sel.toString().length > 0) return;
    if (isPending) return;
    if (clickTimerRef.current) clearTimeout(clickTimerRef.current);
    clickTimerRef.current = window.setTimeout(() => {
      onReply(message);
      clickTimerRef.current = null;
    }, 250);
  }, [message, isPending, onReply]);

  const handleBodyDblClick = useCallback(() => {
    // 取消单击的延迟回复
    if (clickTimerRef.current) {
      clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    if (isPending) return;
    onShowThread(message);
  }, [message, isPending, onShowThread]);

  const handleReplyRefClick = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation(); // 不触发消息正文的 click
      if (message.point_to > 0) {
        onScrollTo(message.point_to);
      }
    },
    [message.point_to, onScrollTo],
  );

  const handleCopy = useCallback(
    (e: React.MouseEvent) => {
      e.stopPropagation();
      onCopy(message.body, message.line_number);
    },
    [message.body, message.line_number, onCopy],
  );

  return (
    <div
      className={[
        'message-item',
        isSending ? 'message-pending' : '',
        isSent ? 'message-sent' : '',
        isFailed ? 'message-failed' : '',
        highlight ? 'message-highlight' : '',
        isReplying ? 'message-replying' : '',
      ]
        .filter(Boolean)
        .join(' ')}
      data-line={message.line_number}
    >
      {!isPending && (
        <div className="message-actions">
          <button className="message-action-btn" onClick={(e) => { e.stopPropagation(); onReply(message); }}>
            回复
          </button>
          <button className="message-action-btn" onClick={(e) => { e.stopPropagation(); onShowThread(message); }}>
            线程
          </button>
          <button className="message-action-btn" onClick={handleCopy}>
            {copied ? '已复制' : '复制'}
          </button>
        </div>
      )}
      <div className="message-header">
        <span className="message-author">@{message.author}</span>
        <span className="message-time">{formatTimestamp(message.timestamp)}</span>
        {statusText && (
          <span className={`message-status message-status-${message._status}`}>
            {statusText}
          </span>
        )}
      </div>
      {replyTarget && (
        <div className="message-reply-ref reply-ref-clickable" onClick={handleReplyRefClick}>
          <span className="reply-author">@{replyTarget.author}:</span>
          {replyTarget.body.length > 60
            ? replyTarget.body.slice(0, 60) + '...'
            : replyTarget.body}
        </div>
      )}
      <div
        className="message-body"
        onClick={handleBodyClick}
        onDoubleClick={handleBodyDblClick}
      >
        {message.body}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: 添加高亮闪烁和可点击回复引用样式**

在 `index.css` 中添加：

```css
/* 消息高亮闪烁 */
.message-highlight {
  animation: highlight-fade 1.5s ease-out;
}

@keyframes highlight-fade {
  0% { background: rgba(79, 195, 247, 0.3); }
  100% { background: transparent; }
}

/* 可点击的回复引用 */
.reply-ref-clickable {
  cursor: pointer;
  transition: background 0.15s;
}

.reply-ref-clickable:hover {
  background: rgba(79, 195, 247, 0.1);
  border-left-color: var(--accent);
}

/* 正在回复的消息 */
.message-replying {
  background: rgba(79, 195, 247, 0.08);
  border-left: 2px solid var(--accent);
}

/* 消息正文可交互 */
.message-body {
  cursor: pointer;
}
```

- [ ] **Step 3: 构建验证**

```bash
cd webui && npm run build
```

- [ ] **Step 4: 提交**

```bash
git add webui/src/components/MessageItem.tsx webui/src/index.css
git commit -m "feat(webui): add click-reply, dblclick-thread, reply-ref-jump, hover-copy to messages"
```

---

### Task 3: MessageList — 跳转到消息 + 高亮闪烁

**Files:**
- Modify: `webui/src/components/MessageList.tsx`
- Modify: `webui/src/hooks/useStore.ts`

- [ ] **Step 1: useStore 新增 highlightLine 状态**

```ts
// useStore.ts — 在 threadMessages 下方新增
highlightLine: number | null;
setHighlightLine: (line: number | null) => void;

// 实现
highlightLine: null,
setHighlightLine: (line) => set({ highlightLine: line }),
```

- [ ] **Step 2: 重构 MessageList 支持跳转和高亮**

```tsx
// MessageList.tsx
import { useState, useEffect, useRef, useMemo, useCallback } from 'react';
import { useStore } from '../hooks/useStore.js';
import { MessageItem } from './MessageItem.js';
import type { Message } from '../lib/types.js';

interface MessageListProps {
  onReply: (m: Message) => void;
  onShowThread: (m: Message) => void;
}

export function MessageList({ onReply, onShowThread }: MessageListProps) {
  const messages = useStore((s) => s.messages);
  const currentChannel = useStore((s) => s.currentChannel);
  const replyTo = useStore((s) => s.replyTo);
  const highlightLine = useStore((s) => s.highlightLine);
  const setHighlightLine = useStore((s) => s.setHighlightLine);
  const listRef = useRef<HTMLDivElement>(null);
  const prevLengthRef = useRef(0);

  // 新消息自动滚动到底部
  useEffect(() => {
    if (messages.length > prevLengthRef.current && listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
    prevLengthRef.current = messages.length;
  }, [messages]);

  // 切换频道时滚动到底部
  useEffect(() => {
    if (listRef.current) {
      listRef.current.scrollTop = listRef.current.scrollHeight;
    }
  }, [currentChannel]);

  // 高亮消失定时器
  useEffect(() => {
    if (highlightLine == null) return;
    const timer = setTimeout(() => setHighlightLine(null), 1500);
    return () => clearTimeout(timer);
  }, [highlightLine, setHighlightLine]);

  // 构建行号到消息的映射
  const msgByLine = useMemo(
    () => new Map(messages.map((m) => [m.line_number, m])),
    [messages],
  );

  // 跳转到指定行号的消息
  const handleScrollTo = useCallback(
    (lineNumber: number) => {
      const el = listRef.current?.querySelector(`[data-line="${lineNumber}"]`);
      if (el) {
        el.scrollIntoView({ behavior: 'smooth', block: 'center' });
        setHighlightLine(lineNumber);
      }
    },
    [setHighlightLine],
  );

  // 复制消息正文（带视觉反馈）
  const [copiedLine, setCopiedLine] = useState<number | null>(null);
  const handleCopy = useCallback((body: string, lineNumber: number) => {
    void navigator.clipboard.writeText(body);
    setCopiedLine(lineNumber);
    setTimeout(() => setCopiedLine(null), 1500);
  }, []);

  if (!currentChannel) {
    return (
      <div className="message-list-empty">选择一个频道开始聊天</div>
    );
  }

  if (messages.length === 0) {
    return (
      <div className="message-list" ref={listRef}>
        <div className="message-list-empty">暂无消息</div>
      </div>
    );
  }

  return (
    <div className="message-list" ref={listRef}>
      {messages.map((msg) => (
        <MessageItem
          key={msg._pendingId ?? msg.line_number}
          message={msg}
          replyTarget={msg.point_to > 0 ? msgByLine.get(msg.point_to) ?? null : null}
          isReplying={replyTo?.line_number === msg.line_number}
          highlight={highlightLine === msg.line_number}
          onReply={onReply}
          onShowThread={onShowThread}
          onScrollTo={handleScrollTo}
          onCopy={handleCopy}
          copied={copiedLine === msg.line_number}
        />
      ))}
    </div>
  );
}
```

- [ ] **Step 3: 构建验证**

```bash
cd webui && npm run build
```

- [ ] **Step 4: 提交**

```bash
git add webui/src/components/MessageList.tsx webui/src/hooks/useStore.ts
git commit -m "feat(webui): add scroll-to-message with highlight and copy support"
```

---

### Task 4: InputArea — Escape 取消回复

**Files:**
- Modify: `webui/src/components/InputArea.tsx`

- [ ] **Step 1: 在 handleKeyDown 中添加 Escape 处理**

在 `InputArea.tsx` 的 `handleKeyDown` 函数中，mention popup 判断之后、Enter 判断之前，加入：

```tsx
if (e.key === 'Escape' && replyTo) {
  e.preventDefault();
  setReplyTo(null);
  return;
}
```

- [ ] **Step 2: 构建验证**

```bash
cd webui && npm run build
```

- [ ] **Step 3: 提交**

```bash
git add webui/src/components/InputArea.tsx
git commit -m "feat(webui): support Escape to cancel reply"
```

---

### Task 5: App.tsx — toggle 回复 + 整合交互

**Files:**
- Modify: `webui/src/App.tsx`

- [ ] **Step 1: 修改 handleReply 支持 toggle**

```tsx
// App.tsx — 替换现有 handleReply
const handleReply = useCallback(
  (msg: Message) => {
    const current = useStore.getState().replyTo;
    if (current && current.line_number === msg.line_number) {
      // 再次点击同一条消息 → 取消回复
      setReplyTo(null);
    } else {
      setReplyTo(msg);
    }
  },
  [setReplyTo],
);
```

- [ ] **Step 2: 构建验证**

```bash
cd webui && npm run build
```

- [ ] **Step 3: 提交**

```bash
git add webui/src/App.tsx
git commit -m "feat(webui): toggle reply on repeated click"
```

---

## Chunk 3: 线程面板改造

### Task 6: ThreadPanel — 扁平时间序展示完整线程树

**Files:**
- Modify: `webui/src/components/ThreadPanel.tsx`
- Modify: `webui/src/App.tsx`

- [ ] **Step 1: 修改 handleShowThread 收集所有层级的回复**

当前 `handleShowThread` 只过滤 `point_to === rootLine`（一层）。改为递归收集整棵树：

```tsx
// App.tsx — 替换现有 handleShowThread
const handleShowThread = useCallback(
  async (msg: Message) => {
    // 沿 point_to 链追溯到真正的 root（而非只上溯一层）
    let rootLine = msg.line_number;
    let current = msg;
    while (current.point_to > 0) {
      const parent = messages.find((m) => m.line_number === current.point_to);
      if (!parent) break; // parent 不在已加载消息中，用当前节点作为 best-effort root
      rootLine = parent.line_number;
      current = parent;
    }
    const rootMsg = messages.find((m) => m.line_number === rootLine) ?? msg;
    setThreadRoot(rootMsg);

    // 收集完整树：root + 所有直接/间接回复
    // 使用 BFS：从 root 开始，找所有 point_to 指向已收集节点的消息
    const treeLines = new Set<number>([rootLine]);
    let changed = true;
    while (changed) {
      changed = false;
      for (const m of messages) {
        if (!treeLines.has(m.line_number) && treeLines.has(m.point_to)) {
          treeLines.add(m.line_number);
          changed = true;
        }
      }
    }

    const threadMsgs = messages
      .filter((m) => treeLines.has(m.line_number))
      .sort((a, b) => a.line_number - b.line_number); // 按行号（时间序）排列

    setThreadMessages(threadMsgs);
  },
  [messages, setThreadRoot, setThreadMessages],
);
```

- [ ] **Step 2: 重构 ThreadPanel 为扁平时间序**

去掉 root/replies 分离逻辑，直接按时间顺序渲染所有消息。每条消息如果有 `point_to`，显示回复引用。

```tsx
// ThreadPanel.tsx
import { useCallback, useMemo } from 'react';
import { useStore } from '../hooks/useStore.js';
import { formatTimestamp } from '../lib/types.js';
import type { Message } from '../lib/types.js';

interface ThreadPanelProps {
  onReplyInThread: (m: Message) => void;
}

export function ThreadPanel({ onReplyInThread }: ThreadPanelProps) {
  const threadRoot = useStore((s) => s.threadRoot);
  const setThreadRoot = useStore((s) => s.setThreadRoot);
  const threadMessages = useStore((s) => s.threadMessages);

  const msgByLine = useMemo(
    () => new Map(threadMessages.map((m) => [m.line_number, m])),
    [threadMessages],
  );

  const handleReply = useCallback(
    (e: React.MouseEvent, msg: Message) => {
      e.stopPropagation();
      onReplyInThread(msg);
    },
    [onReplyInThread],
  );

  if (!threadRoot) return null;

  return (
    <div className="thread-panel">
      <div className="thread-header">
        <span>线程: L{threadRoot.line_number}</span>
        <button className="thread-close-btn" onClick={() => setThreadRoot(null)}>
          ×
        </button>
      </div>
      <div className="thread-messages">
        {threadMessages.map((msg) => {
          const replyTarget = msg.point_to > 0 ? msgByLine.get(msg.point_to) ?? null : null;
          const isRoot = msg.line_number === threadRoot.line_number;
          return (
            <div key={msg.line_number} className={`thread-msg ${isRoot ? 'thread-msg-root' : ''}`}>
              <div className="message-header">
                <span className="message-author">@{msg.author}</span>
                <span className="message-time">{formatTimestamp(msg.timestamp)}</span>
              </div>
              {replyTarget && (
                <div className="message-reply-ref">
                  <span className="reply-author">@{replyTarget.author}:</span>
                  {replyTarget.body.length > 60
                    ? replyTarget.body.slice(0, 60) + '...'
                    : replyTarget.body}
                </div>
              )}
              <div className="message-body">{msg.body}</div>
              <button
                className="message-action-btn"
                style={{ marginTop: 4 }}
                onClick={(e) => handleReply(e, msg)}
              >
                回复
              </button>
            </div>
          );
        })}
      </div>
    </div>
  );
}
```

- [ ] **Step 3: 更新 ThreadPanel CSS — 去掉缩进，加 root 样式**

在 `index.css` 中替换 `.thread-msg-indent` 为：

```css
/* 线程根消息 */
.thread-msg-root {
  border-left: 2px solid var(--accent);
  padding-left: 8px;
}
```

删除 `.thread-msg-indent` 规则。

- [ ] **Step 4: 构建验证**

```bash
cd webui && npm run build
```

- [ ] **Step 5: 提交**

```bash
git add webui/src/components/ThreadPanel.tsx webui/src/App.tsx webui/src/index.css
git commit -m "feat(webui): flat timeline thread panel with full tree collection"
```
