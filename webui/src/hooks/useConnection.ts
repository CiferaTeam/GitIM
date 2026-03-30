import { useEffect, useRef, useCallback } from 'react';
import * as api from '../lib/http-client.js';
import { useStore } from './useStore.js';
import type { Message, Channel, ApiResponse, PollChange } from '../lib/types.js';

const POLL_INTERVAL = 3000;

/** 将 sidebar 显示名转为 API channel 名：DM "alice--bob" → "dm:alice,bob" */
function toApiChannel(name: string): string {
  if (name.includes('--')) {
    const parts = name.split('--');
    return `dm:${parts[0]},${parts[1]}`;
  }
  return name;
}

/** 解析 /api/channels 响应 — 兼容新格式（对象数组）和旧格式（字符串数组） */
function parseChannelsResponse(data: Record<string, unknown>): Channel[] {
  const raw = data.channels as unknown[];
  if (!Array.isArray(raw) || raw.length === 0) return [];
  if (typeof raw[0] === 'string') {
    // 旧格式：字符串数组
    return (raw as string[]).map((name) => ({
      name,
      kind: name.includes('--') ? 'dm' as const : 'channel' as const,
      unreadCount: 0,
      members: [],
    }));
  }
  // 新格式：对象数组 {name, kind, members}
  return (raw as Array<{ name: string; kind: string; members?: string[] }>).map((ch) => ({
    name: ch.name,
    kind: (ch.kind === 'dm' ? 'dm' : 'channel') as 'channel' | 'dm',
    unreadCount: 0,
    members: ch.members ?? [],
  }));
}

/** HTTP 轮询连接管理 hook */
export function useConnection() {
  const {
    setConnected,
    setCurrentUser,
    setUsers,
    setChannels,
    currentChannel,
    selectChannel,
    setMessages,
    incrementUnread,
  } = useStore();

  // 用 ref 跟踪当前频道，避免 interval 闭包陷阱
  const currentChannelRef = useRef(currentChannel);
  currentChannelRef.current = currentChannel;

  const commitIdRef = useRef<string | undefined>(undefined);
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null);

  /** request 适配层 — 保持 App.tsx 调用接口不变 */
  const request = useCallback(
    async (method: string, params: Record<string, unknown> = {}): Promise<ApiResponse> => {
      switch (method) {
        case 'send':
          return api.send(
            params.channel as string,
            params.body as string,
            params.author as string | undefined,
            params.reply_to as number | undefined,
          );
        case 'read':
          return api.read(
            params.channel as string,
            params.limit as number | undefined,
          );
        case 'thread':
          return api.thread(
            params.channel as string,
            params.line as number,
          );
        case 'me':
          return api.me();
        case 'channels':
          return api.channels();
        case 'users':
          return api.users();
        case 'poll':
          return api.poll(params.since as string | undefined);
        default:
          return { ok: false, error: `Unknown method: ${method}` };
      }
    },
    [],
  );

  // 加载频道消息
  const loadMessages = useCallback(
    async (channel: string) => {
      const res = await api.read(channel, 200);
      if (res.ok && res.data) {
        setMessages((res.data.entries as unknown as Message[]) || []);
      }
    },
    [setMessages],
  );

  useEffect(() => {
    let disposed = false;

    async function init() {
      // 并行获取初始数据
      const [meRes, chRes, usersRes] = await Promise.all([
        api.me(),
        api.channels(),
        api.users(),
      ]);

      if (disposed) return;

      // 任何一个成功即认为 API 可达
      if (meRes.ok || chRes.ok || usersRes.ok) {
        setConnected(true);
      }

      if (meRes.ok && meRes.data) {
        setCurrentUser(meRes.data.handler as string);
      }

      if (chRes.ok && chRes.data) {
        const channels: Channel[] = parseChannelsResponse(chRes.data);
        setChannels(channels);

        // 默认选中第一个频道并加载消息
        if (channels.length > 0 && !currentChannelRef.current) {
          selectChannel(channels[0].name);
          const readRes = await api.read(toApiChannel(channels[0].name), 200);
          if (!disposed && readRes.ok && readRes.data) {
            setMessages((readRes.data.entries as unknown as Message[]) || []);
          }
        }
      }

      if (usersRes.ok && usersRes.data) {
        setUsers((usersRes.data.users as unknown as string[]) || []);
      }

      // 获取初始 commit_id（不带 since 参数）
      const pollRes = await api.poll();
      if (!disposed && pollRes.ok && pollRes.data) {
        commitIdRef.current = pollRes.data.commit_id as string | undefined;
      }

      if (disposed) return;

      // 开始轮询
      intervalRef.current = setInterval(async () => {
        const res = await api.poll(commitIdRef.current);
        if (!res.ok) {
          setConnected(false);
          return;
        }
        setConnected(true);

        const data = res.data;
        if (!data) return;

        // 更新 commit_id
        if (data.commit_id) {
          commitIdRef.current = data.commit_id as string;
        }

        const changes = (data.changes as unknown as PollChange[]) || [];
        if (changes.length === 0) return;

        let needRefreshUsers = false;
        let needRefreshChannels = false;
        const currentChannels = useStore.getState().channels;

        for (const change of changes) {
          if (change.kind === 'user') {
            needRefreshUsers = true;
            continue;
          }
          // 检查是否为新 channel（不在当前列表中）
          if (change.channel && !currentChannels.some((c) => c.name === change.channel)) {
            needRefreshChannels = true;
          }
          // channel / dm 变更
          if (change.channel === currentChannelRef.current) {
            // 当前频道 → 重新加载消息（DM 需要 dm: 前缀）
            const readRes = await api.read(toApiChannel(change.channel), 200);
            if (readRes.ok && readRes.data) {
              setMessages((readRes.data.entries as unknown as Message[]) || []);
            }
          } else if (change.channel) {
            // 其他频道 → 增加未读
            incrementUnread(change.channel);
          }
        }

        // 有新 channel/DM 出现 → 重新拉取频道列表
        if (needRefreshChannels) {
          const chRes = await api.channels();
          if (chRes.ok && chRes.data) {
            const freshChannels = parseChannelsResponse(chRes.data);
            // 保留已有 channel 的 unreadCount
            const oldMap = new Map(currentChannels.map((c) => [c.name, c]));
            const refreshed: Channel[] = freshChannels.map((ch) => {
              const old = oldMap.get(ch.name);
              return old ? { ...old, members: ch.members } : { ...ch, unreadCount: 1 };
            });
            setChannels(refreshed);
          }
        }

        if (needRefreshUsers) {
          const usersRes = await api.users();
          if (usersRes.ok && usersRes.data) {
            setUsers((usersRes.data.users as unknown as string[]) || []);
          }
        }
      }, POLL_INTERVAL);
    }

    void init();

    return () => {
      disposed = true;
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { request, loadMessages };
}
