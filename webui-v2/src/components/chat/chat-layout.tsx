import { useCallback } from "react";
import { useChatStore } from "../../hooks/use-chat-store";
import * as mockClient from "../../lib/mock/client";
import type { Message } from "../../lib/types";
import { ChatHeader } from "./header";
import { MessageList } from "./message-list";
import { Sidebar } from "./sidebar";

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
  const threadRoot = useChatStore((s) => s.threadRoot);

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
        // Add a new DM channel to the store so it appears in the sidebar
        setChannels([
          ...channels,
          { name: displayName, kind: "dm", unreadCount: 0, members: parts },
        ]);
      }
      await handleChannelSelect(displayName);
    },
    [currentUser, channels, setChannels, handleChannelSelect]
  );

  const handleSend = useCallback(
    async (body: string, pointTo?: number) => {
      if (!currentChannel) return;
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

  // handleSend is consumed by Task 11 (input area)
  void handleSend;

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

        {/* Input area — placeholder until Task 11 */}
        <div className="border-t p-3">
          <div className="h-10 rounded-md border bg-muted/30 flex items-center px-3 text-sm text-muted-foreground">
            Message input (Task 11)
          </div>
        </div>
      </div>

      {/* Right: thread panel — placeholder until Task 12 */}
      {threadRoot && (
        <div className="w-80 shrink-0 border-l flex flex-col">
          <div className="h-12 border-b flex items-center px-4 text-sm font-medium">
            Thread
          </div>
          <div className="flex-1 overflow-y-auto p-4 text-sm text-muted-foreground">
            Thread panel (Task 12)
          </div>
        </div>
      )}
    </div>
  );
}
