// GitIM WebUI 主逻辑

// ===== 状态 =====
const state = {
  channels: [],
  users: [],
  currentChannel: null,
  messages: [],       // 当前频道的消息
  currentUser: null,  // {handler, display_name}
  connected: false,
  replyTo: null,      // 回复目标消息
  unreadCounts: {},   // 频道未读数
  mentionIndex: -1,   // @提及选中索引
  mentionFiltered: [],// 过滤后的用户列表
};

// 缓存 DOM 元素
const dom = {
  channelTitle: document.getElementById('channel-title'),
  channelList: document.getElementById('channel-list'),
  userList: document.getElementById('user-list'),
  messages: document.getElementById('messages'),
  msgInput: document.getElementById('msg-input'),
  sendBtn: document.getElementById('send-btn'),
  replyIndicator: document.getElementById('reply-indicator'),
  replyText: document.getElementById('reply-text'),
  replyCancel: document.getElementById('reply-cancel'),
  mentionPopup: document.getElementById('mention-popup'),
  connectionStatus: document.getElementById('connection-status'),
  currentUserEl: document.getElementById('current-user'),
  threadPanel: document.getElementById('thread-panel'),
  threadTitle: document.getElementById('thread-title'),
  threadClose: document.getElementById('thread-close'),
  threadMessages: document.getElementById('thread-messages'),
};

// ===== API 客户端 =====
async function api(path, options = {}) {
  try {
    const res = await fetch(path, options);
    const data = await res.json();
    if (!data.ok) {
      console.error(`API 错误 [${path}]:`, data.error);
      return null;
    }
    return data.data;
  } catch (err) {
    console.error(`请求失败 [${path}]:`, err);
    return null;
  }
}

// ===== WebSocket =====
let ws = null;
let wsReconnectTimer = null;

function connectWebSocket() {
  const protocol = location.protocol === 'https:' ? 'wss:' : 'ws:';
  ws = new WebSocket(`${protocol}//${location.host}/ws`);

  ws.onopen = () => {
    console.log('[ws] 已连接');
    if (wsReconnectTimer) {
      clearTimeout(wsReconnectTimer);
      wsReconnectTimer = null;
    }
  };

  ws.onmessage = (ev) => {
    try {
      const msg = JSON.parse(ev.data);
      handlePushEvent(msg);
    } catch (e) {
      console.error('[ws] 解析失败:', e);
    }
  };

  ws.onclose = () => {
    console.log('[ws] 断开');
    updateConnectionStatus(false);
    // 自动重连
    wsReconnectTimer = setTimeout(connectWebSocket, 3000);
  };

  ws.onerror = (err) => {
    console.error('[ws] 错误:', err);
  };
}

// ===== 推送事件处理 =====
function handlePushEvent(msg) {
  if (msg.event === 'connected') {
    updateConnectionStatus(msg.daemon);
    return;
  }

  if (msg.event === 'thread_changed') {
    // 频道有新消息
    if (msg.channel === state.currentChannel) {
      // 当前频道 → 重新加载消息
      loadMessages(state.currentChannel);
    } else {
      // 其他频道 → 增加未读数
      state.unreadCounts[msg.channel] = (state.unreadCounts[msg.channel] || 0) + 1;
      renderSidebar();
    }
  }
}

// ===== 初始化 =====
async function init() {
  // 获取当前用户
  const meData = await api('/api/me');
  if (meData) {
    state.currentUser = meData;
    dom.currentUserEl.textContent = `@${meData.handler}`;
  } else {
    dom.currentUserEl.textContent = '未连接';
  }

  // 获取频道列表
  const channelsData = await api('/api/channels');
  if (channelsData) {
    state.channels = channelsData.channels || [];
  }

  // 获取用户列表
  const usersData = await api('/api/users');
  if (usersData) {
    state.users = usersData.users || [];
  }

  renderSidebar();

  // 默认选择第一个频道
  if (state.channels.length > 0) {
    selectChannel(state.channels[0]);
  } else {
    dom.messages.innerHTML = '<div class="empty-state">暂无频道</div>';
  }

  // 连接 WebSocket
  connectWebSocket();

  // 绑定事件
  bindEvents();
}

// ===== 事件绑定 =====
function bindEvents() {
  // 发送按钮
  dom.sendBtn.addEventListener('click', () => {
    const body = dom.msgInput.value.trim();
    if (body) sendMessage(body);
  });

  // 输入框 Enter 发送
  dom.msgInput.addEventListener('keydown', (e) => {
    // @提及导航
    if (!dom.mentionPopup.classList.contains('hidden')) {
      if (e.key === 'ArrowDown') {
        e.preventDefault();
        state.mentionIndex = Math.min(state.mentionIndex + 1, state.mentionFiltered.length - 1);
        renderMentionPopup();
        return;
      }
      if (e.key === 'ArrowUp') {
        e.preventDefault();
        state.mentionIndex = Math.max(state.mentionIndex - 1, 0);
        renderMentionPopup();
        return;
      }
      if (e.key === 'Enter' || e.key === 'Tab') {
        e.preventDefault();
        if (state.mentionFiltered[state.mentionIndex]) {
          insertMention(state.mentionFiltered[state.mentionIndex]);
        }
        return;
      }
      if (e.key === 'Escape') {
        hideMentionPopup();
        return;
      }
    }

    // Enter 发送（Shift+Enter 换行）
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      const body = dom.msgInput.value.trim();
      if (body) sendMessage(body);
    }
  });

  // @提及检测
  dom.msgInput.addEventListener('input', () => {
    detectMention();
    // 自动调整高度
    dom.msgInput.style.height = 'auto';
    dom.msgInput.style.height = Math.min(dom.msgInput.scrollHeight, 120) + 'px';
  });

  // 取消回复
  dom.replyCancel.addEventListener('click', clearReply);

  // 关闭线程面板
  dom.threadClose.addEventListener('click', () => {
    dom.threadPanel.classList.add('hidden');
  });

  // 消息列表事件委托
  dom.messages.addEventListener('click', (e) => {
    const replyBtn = e.target.closest('.btn-reply');
    if (replyBtn) {
      setReply(parseInt(replyBtn.dataset.line, 10));
      return;
    }
    const threadBtn = e.target.closest('.btn-thread');
    if (threadBtn) {
      showThread(threadBtn.dataset.channel, parseInt(threadBtn.dataset.line, 10));
      return;
    }
    const replyRef = e.target.closest('.msg-reply-ref[data-goto]');
    if (replyRef) {
      scrollToMessage(parseInt(replyRef.dataset.goto, 10));
      return;
    }
  });

  // 侧边栏频道事件委托
  dom.channelList.addEventListener('click', (e) => {
    const li = e.target.closest('li[data-channel]');
    if (li) selectChannel(li.dataset.channel);
  });

  // 提及弹窗事件委托
  dom.mentionPopup.addEventListener('click', (e) => {
    const item = e.target.closest('.mention-item[data-user]');
    if (item) insertMention(item.dataset.user);
  });
}

// ===== 频道切换 =====
async function selectChannel(channel) {
  state.currentChannel = channel;
  state.replyTo = null;
  state.unreadCounts[channel] = 0;
  dom.channelTitle.textContent = `#${channel}`;
  dom.replyIndicator.classList.add('hidden');
  renderSidebar();
  await loadMessages(channel);
  // 滚动到底部
  dom.messages.scrollTop = dom.messages.scrollHeight;
  dom.msgInput.focus();
}

// ===== 消息加载 =====
async function loadMessages(channel, limit = 50, since = 0) {
  const data = await api(`/api/messages?channel=${encodeURIComponent(channel)}&limit=${limit}&since=${since}`);
  if (data) {
    state.messages = data.messages || [];
    renderMessages(state.messages);
  } else {
    dom.messages.innerHTML = '<div class="empty-state">无法加载消息</div>';
  }
}

// ===== 消息渲染 =====
function renderMessages(messages) {
  if (messages.length === 0) {
    dom.messages.innerHTML = '<div class="empty-state">暂无消息，发送第一条吧</div>';
    return;
  }

  // 建立行号 → 消息映射（用于显示回复引用）
  const msgMap = new Map();
  for (const msg of messages) {
    msgMap.set(msg.line_number, msg);
  }

  dom.messages.innerHTML = messages.map((msg) => renderMessage(msg, msgMap)).join('');

  // 滚动到底部
  dom.messages.scrollTop = dom.messages.scrollHeight;
}

function renderMessage(msg, msgMap) {
  const time = formatTimestamp(msg.timestamp);
  const author = escapeHtml(msg.author);
  const body = escapeHtml(msg.body);
  const lineNum = msg.line_number;

  // 回复引用
  let replyHtml = '';
  if (msg.point_to > 0) {
    const parent = msgMap.get(msg.point_to);
    if (parent) {
      const parentAuthor = escapeHtml(parent.author);
      const parentBody = escapeHtml(parent.body.slice(0, 60));
      replyHtml = `<div class="msg-reply-ref" data-goto="${parent.line_number}" title="跳转到原消息">@${parentAuthor}: ${parentBody}</div>`;
    } else {
      replyHtml = `<div class="msg-reply-ref">回复 L${msg.point_to}</div>`;
    }
  }

  return `
    <div class="message" id="msg-${lineNum}" data-line="${lineNum}">
      <div class="msg-header">
        <span class="msg-author">@${author}</span>
        <span class="msg-time">${time}</span>
      </div>
      ${replyHtml}
      <div class="msg-body">${body}</div>
      <div class="msg-actions">
        <button class="btn-reply" data-line="${lineNum}" title="回复">回复</button>
        <button class="btn-thread" data-line="${lineNum}" data-channel="${escapeHtml(state.currentChannel)}" title="查看引用链">线程</button>
      </div>
    </div>
  `;
}

// ===== 消息发送 =====
async function sendMessage(body) {
  if (!state.currentChannel || !state.currentUser) return;

  dom.sendBtn.disabled = true;
  const result = await api('/api/send', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({
      channel: state.currentChannel,
      body,
      author: state.currentUser.handler,
      reply_to: state.replyTo ? state.replyTo.line_number : null,
    }),
  });

  dom.sendBtn.disabled = false;

  if (result !== null) {
    dom.msgInput.value = '';
    dom.msgInput.style.height = 'auto';
    clearReply();
    // 重新加载消息
    await loadMessages(state.currentChannel);
  }
}

// ===== 回复 =====
function setReply(lineNumber) {
  const msg = state.messages.find((m) => m.line_number === lineNumber);
  if (!msg) return;

  state.replyTo = msg;
  dom.replyText.textContent = `回复 @${msg.author}: ${msg.body.slice(0, 50)}${msg.body.length > 50 ? '…' : ''}`;
  dom.replyIndicator.classList.remove('hidden');
  dom.msgInput.focus();
}

function clearReply() {
  state.replyTo = null;
  dom.replyIndicator.classList.add('hidden');
}

// ===== 跳转到消息 =====
function scrollToMessage(lineNumber) {
  const el = document.getElementById(`msg-${lineNumber}`);
  if (el) {
    el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    el.style.background = 'var(--bg-tertiary)';
    setTimeout(() => { el.style.background = ''; }, 1500);
  }
}

// ===== 线程链 =====
async function showThread(channel, lineNumber) {
  dom.threadTitle.textContent = `引用链: L${lineNumber}`;
  dom.threadPanel.classList.remove('hidden');
  dom.threadMessages.innerHTML = '<div class="empty-state">加载中…</div>';

  const data = await api(`/api/thread?channel=${encodeURIComponent(channel)}&line=${lineNumber}`);
  if (data && data.messages) {
    renderThread(data.messages, lineNumber);
  } else {
    dom.threadMessages.innerHTML = '<div class="empty-state">无法加载线程</div>';
  }
}

function renderThread(messages, rootLine) {
  if (messages.length === 0) {
    dom.threadMessages.innerHTML = '<div class="empty-state">无消息</div>';
    return;
  }

  dom.threadMessages.innerHTML = messages.map((msg) => {
    const isRoot = msg.line_number === rootLine || msg.point_to === 0;
    const time = formatTimestamp(msg.timestamp);
    return `
      <div class="thread-msg ${isRoot ? 'root' : 'reply'}">
        <div class="msg-header">
          <span class="msg-author">@${escapeHtml(msg.author)}</span>
          <span class="msg-time">${time}</span>
        </div>
        <div class="msg-body">${escapeHtml(msg.body)}</div>
      </div>
    `;
  }).join('');
}

// ===== @提及 =====
function detectMention() {
  const input = dom.msgInput;
  const text = input.value;
  const cursor = input.selectionStart;

  // 向前找 @ 符号
  let atPos = -1;
  for (let i = cursor - 1; i >= 0; i--) {
    if (text[i] === '@') {
      atPos = i;
      break;
    }
    if (text[i] === ' ' || text[i] === '\n') break;
  }

  if (atPos === -1) {
    hideMentionPopup();
    return;
  }

  const filter = text.slice(atPos + 1, cursor).toLowerCase();
  state.mentionFiltered = state.users.filter((u) => u.toLowerCase().includes(filter));
  state.mentionIndex = 0;

  if (state.mentionFiltered.length === 0) {
    hideMentionPopup();
    return;
  }

  showMentionPopup();
}

function showMentionPopup() {
  dom.mentionPopup.classList.remove('hidden');
  renderMentionPopup();
}

function hideMentionPopup() {
  dom.mentionPopup.classList.add('hidden');
  state.mentionFiltered = [];
  state.mentionIndex = -1;
}

function renderMentionPopup() {
  dom.mentionPopup.innerHTML = state.mentionFiltered.map((user, i) =>
    `<div class="mention-item ${i === state.mentionIndex ? 'active' : ''}"
          data-user="${escapeHtml(user)}"
     >@${escapeHtml(user)}</div>`
  ).join('');
}

function insertMention(username) {
  const input = dom.msgInput;
  const text = input.value;
  const cursor = input.selectionStart;

  // 找到 @ 位置
  let atPos = -1;
  for (let i = cursor - 1; i >= 0; i--) {
    if (text[i] === '@') { atPos = i; break; }
    if (text[i] === ' ' || text[i] === '\n') break;
  }
  if (atPos === -1) return;

  const before = text.slice(0, atPos);
  const after = text.slice(cursor);
  input.value = `${before}@${username} ${after}`;
  const newCursor = atPos + username.length + 2;
  input.setSelectionRange(newCursor, newCursor);
  hideMentionPopup();
  input.focus();
}

// ===== 侧边栏渲染 =====
function renderSidebar() {
  // 频道列表
  dom.channelList.innerHTML = state.channels.map((ch) => {
    const isActive = ch === state.currentChannel ? 'active' : '';
    const unread = state.unreadCounts[ch] || 0;
    const badge = unread > 0 ? `<span class="unread-badge">${unread}</span>` : '';
    return `<li class="${isActive}" data-channel="${escapeHtml(ch)}"># ${escapeHtml(ch)}${badge}</li>`;
  }).join('');

  // 用户列表
  dom.userList.innerHTML = state.users.map((u) => {
    const isMe = state.currentUser && u === state.currentUser.handler;
    return `<li><span class="user-status">●</span>${escapeHtml(u)}${isMe ? ' (我)' : ''}</li>`;
  }).join('');
}

// ===== 连接状态 =====
function updateConnectionStatus(connected) {
  state.connected = connected;
  dom.connectionStatus.className = `status-dot ${connected ? 'connected' : 'disconnected'}`;
  dom.connectionStatus.title = connected ? '已连接' : '未连接';
}

// ===== 工具函数 =====

// 解析紧凑时间格式: 20260317T120000Z → 12:00
function formatTimestamp(ts) {
  if (!ts || ts.length < 15) return ts || '';
  const h = ts.slice(9, 11);
  const m = ts.slice(11, 13);
  return `${h}:${m}`;
}

// HTML 转义（防 XSS）
function escapeHtml(text) {
  if (!text) return '';
  const div = document.createElement('div');
  div.textContent = text;
  return div.innerHTML;
}

// ===== 启动 =====
init();
