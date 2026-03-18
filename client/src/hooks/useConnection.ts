import { useEffect, useRef, useCallback } from 'react';
import { WsClient } from '../lib/ws-client.js';
import { useStore } from './useStore.js';
import type { Message, Channel } from '../lib/types.js';

/** WebSocket 连接管理 hook */
export function useConnection() {
  const clientRef = useRef<WsClient | null>(null);
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

  // 用 ref 跟踪当前频道，避免 push handler 闭包陷阱
  const currentChannelRef = useRef(currentChannel);
  currentChannelRef.current = currentChannel;

  const request = useCallback(
    async (method: string, params: Record<string, unknown> = {}) => {
      if (!clientRef.current) {
        return { id: 0, ok: false, error: '客户端未初始化' };
      }
      return clientRef.current.request(method, params);
    },
    [],
  );

  // 加载频道消息
  const loadMessages = useCallback(
    async (channel: string) => {
      const res = await request('read', { channel, limit: 200 });
      if (res.ok && res.data) {
        setMessages((res.data.messages as Message[]) || []);
      }
    },
    [request, setMessages],
  );

  useEffect(() => {
    const wsUrl = `ws://${location.host}/ws`;
    const client = new WsClient(wsUrl);
    clientRef.current = client;

    client.onConnectionChange = (connected) => {
      setConnected(connected);
      if (connected) {
        // 初始化加载
        void (async () => {
          // 获取当前用户
          const meRes = await client.request('me');
          if (meRes.ok && meRes.data) {
            setCurrentUser(meRes.data.handler as string);
          }

          // 获取频道列表
          const chRes = await client.request('channels');
          if (chRes.ok && chRes.data) {
            const channels = (chRes.data.channels as Channel[]) || [];
            setChannels(channels);
            // 默认选中第一个频道
            if (channels.length > 0 && !currentChannelRef.current) {
              selectChannel(channels[0].name);
              const readRes = await client.request('read', {
                channel: channels[0].name,
                limit: 200,
              });
              if (readRes.ok && readRes.data) {
                setMessages((readRes.data.messages as Message[]) || []);
              }
            }
          }

          // 获取用户列表
          const usersRes = await client.request('users');
          if (usersRes.ok && usersRes.data) {
            setUsers((usersRes.data.users as string[]) || []);
          }
        })();
      }
    };

    // 推送事件处理
    client.onPush((event) => {
      if (event.event === 'thread_changed') {
        const ch = event.channel;
        if (ch === currentChannelRef.current) {
          // 当前频道更新 → 重新加载消息
          void (async () => {
            const res = await client.request('read', { channel: ch, limit: 200 });
            if (res.ok && res.data) {
              setMessages((res.data.messages as Message[]) || []);
            }
          })();
        } else {
          // 其他频道 → 增加未读数
          incrementUnread(ch);
        }
      }
    });

    client.connect();

    return () => {
      client.disconnect();
      clientRef.current = null;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return { request, loadMessages };
}
