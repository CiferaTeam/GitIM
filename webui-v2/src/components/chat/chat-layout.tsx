import { useCallback } from "react";
import { useChatStore } from "../../hooks/use-chat-store";
import * as mockClient from "../../lib/mock/client";
import type { Message } from "../../lib/types";
import { ChatHeader } from "./header";
import { InputArea } from "./input-area";
import { MessageList } from "./message-list";
import { Sidebar } from "./sidebar";
import { ThreadPanel } from "./thread-panel";

/** "alice--lewis" → "dm:alice,lewis" */
function toApiChannel(displayName: string): string {
  if (displayName.includes("--")) {
    const parts = displayName.split("--");
    return `dm:${parts.join(",")}`;
  }
  return displayName;
}

export function ChatLayout() {
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const currentUser = useChatStore((s) => s.currentUser);

  const selectChannel = useChatStore((s) => s.selectChannel);
  const clearUnread = useChatStore((s) => s.clearUnread);
  const setMessages = useChatStore((s) => s.setMessages);
  const addPendingMessage = useChatStore((s) => s.addPendingMessage);
  const markPendingSent = useChatStore((s) => s.markPendingSent);
  const markPendingFailed = useChatStore((s) => s.markPendingFailed);
  const setReplyTo = useChatStore((s) => s.setReplyTo);
  const setThreadRoot = useChatStore((s) => s.setThreadRoot);
  const setThreadMessages = useChatStore((s) => s.setThreadMessages);
  const setChannels = useChatStore((s) => s.setChannels);

  const handleChannelSelect = useCallback(
    async (name: string) => {
      selectChannel(name);
      clearUnread(name);
      setMessages([]);
      setThreadRoot(null);
      const apiChannel = toApiChannel(name);
      const res = await mockClient.read(apiChannel);
      if (res.ok && res.data) {
        setMessages(res.data.entries as Message[]);
      }
    },
    [selectChannel, clearUnread, setMessages, setThreadRoot]
  );

  const handleStartDm = useCallback(
    async (targetUser: string) => {
      const parts = [currentUser, targetUser].sort();
      const displayName = parts.join("--");
      const exists = channels.some((c) => c.name === displayName);
      if (!exists) {
        const newChannel = { name: displayName, kind: "dm" as const, unreadCount: 0, members: parts };
        // Register in the mock client so the poll loop doesn't overwrite it
        mockClient.addChannel(newChannel);
        // Add to the store so it appears in the sidebar immediately
        setChannels([...channels, newChannel]);
      }
      await handleChannelSelect(displayName);
    },
    [currentUser, channels, setChannels, handleChannelSelect]
  );

  const handleSend = useCallback(
    async (body: string, pointTo: number = 0) => {
      if (!currentChannel) return { ok: false, error: "No channel selected" };
      const pendingId = `pending-${Date.now()}`;
      const pending: Message = {
        line_number: -1,
        point_to: pointTo ?? 0,
        author: currentUser,
        timestamp: new Date()
          .toISOString()
          .replace(/[-:]/g, "")
          .replace(/\.\d+/, ""),
        body,
        _status: "sending",
        _pendingId: pendingId,
      };
      addPendingMessage(pending);

      const apiChannel = toApiChannel(currentChannel);
      const res = await mockClient.send(
        apiChannel,
        body,
        currentUser,
        pointTo
      );
      if (res.ok && res.data) {
        markPendingSent(pendingId, res.data.line_number as number);
      } else {
        markPendingFailed(pendingId);
      }
      return res;
    },
    [currentChannel, currentUser, addPendingMessage, markPendingSent, markPendingFailed]
  );

  const handleReply = useCallback(
    (msg: Message) => {
      setReplyTo(msg);
    },
    [setReplyTo]
  );

  const handleShowThread = useCallback(
    async (msg: Message) => {
      if (!currentChannel) return;
      const apiChannel = toApiChannel(currentChannel);
      const res = await mockClient.thread(apiChannel, msg.line_number);
      if (res.ok && res.data) {
        const entries = res.data.entries as Message[];
        const root = entries[0] ?? msg;
        setThreadRoot(root);
        setThreadMessages(entries);
      }
    },
    [currentChannel, setThreadRoot, setThreadMessages]
  );

  return (
    <div className="flex h-full overflow-hidden">
      {/* Left: sidebar */}
      <Sidebar
        onChannelSelect={handleChannelSelect}
        onStartDm={handleStartDm}
      />

      {/* Center: main content */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <ChatHeader onStartDm={handleStartDm} />

        {/* Message area */}
        <MessageList onReply={handleReply} onShowThread={handleShowThread} />

        <InputArea onSend={handleSend} />
      </div>

      {/* Right: thread panel */}
      <ThreadPanel onReplyInThread={handleReply} />
    </div>
  );
}
