import { useCallback, useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useChatStore } from "../../hooks/use-chat-store";
import * as client from "../../lib/client";
import type { Message } from "../../lib/types";
import { ChatHeader } from "./header";
import { InputArea } from "./input-area";
import { MessageList } from "./message-list";
import { Sidebar } from "./sidebar";
import { ThreadPanel } from "./thread-panel";
import { UserCard } from "./user-card";

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
  const setPendingScrollLine = useChatStore((s) => s.setPendingScrollLine);
  const pushNav = useChatStore((s) => s.pushNav);
  const navHistory = useChatStore((s) => s.navHistory);

  // UserCard popover state
  const [userCardHandler, setUserCardHandler] = useState<string | null>(null);
  const [userCardPosition, setUserCardPosition] = useState<{ x: number; y: number } | null>(null);

  const handleChannelSelect = useCallback(
    async (name: string) => {
      selectChannel(name);
      clearUnread(name);
      setMessages([]);
      setThreadRoot(null);
      const apiChannel = toApiChannel(name);
      const res = await client.read(apiChannel, 50);
      // Guard: discard result if the user switched channels during the await
      if (res.ok && res.data && useChatStore.getState().currentChannel === name) {
        setMessages(res.data.entries as Message[]);
      }
    },
    [selectChannel, clearUnread, setMessages, setThreadRoot]
  );

  // Auto-select "general" when entering chat with no channel focused
  useEffect(() => {
    if (currentChannel) return;
    const general = channels.find((c) => c.name === "general");
    if (general) {
      handleChannelSelect("general");
    }
  }, [channels, currentChannel, handleChannelSelect]);

  const handleStartDm = useCallback(
    async (targetUser: string) => {
      const parts = [currentUser, targetUser].sort();
      const displayName = parts.join("--");
      const exists = channels.some((c) => c.name === displayName);
      if (!exists) {
        const newChannel = { name: displayName, kind: "dm" as const, unreadCount: 0, members: parts };
        // Add to the store so it appears in the sidebar immediately
        // DM will be created on the backend when the first message is sent
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
      const res = await client.send(
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
      const res = await client.thread(apiChannel, msg.line_number);
      if (res.ok && res.data) {
        const entries = res.data.entries as Message[];
        const root = entries[0] ?? msg;
        setThreadRoot(root);
        setThreadMessages(entries);
      }
    },
    [currentChannel, setThreadRoot, setThreadMessages]
  );

  // --- Interactive fragment handlers ---

  const handleMentionClick = useCallback(
    (handler: string, event: React.MouseEvent) => {
      setUserCardHandler(handler);
      setUserCardPosition({ x: event.clientX, y: event.clientY });
    },
    []
  );

  const handleUserProfileClick = useCallback(
    (handler: string, event: React.MouseEvent) => {
      setUserCardHandler(handler);
      setUserCardPosition({ x: event.clientX, y: event.clientY });
    },
    []
  );

  const getScrollTop = useCallback(() => {
    const el = document.querySelector("[data-message-scroll]");
    return el ? el.scrollTop : 0;
  }, []);

  const handleChannelClick = useCallback(
    (channel: string) => {
      // Push current location onto nav stack before jumping
      if (currentChannel) {
        pushNav({ channel: currentChannel, scrollTop: getScrollTop() });
      }
      handleChannelSelect(channel);
    },
    [currentChannel, pushNav, getScrollTop, handleChannelSelect]
  );

  const handleMessageLinkClick = useCallback(
    (channel: string, line: number) => {
      // Push current location onto nav stack before jumping
      if (currentChannel) {
        pushNav({ channel: currentChannel, scrollTop: getScrollTop() });
      }
      // Set pending scroll target BEFORE switching channel
      setPendingScrollLine(line);
      handleChannelSelect(channel);
    },
    [currentChannel, pushNav, getScrollTop, setPendingScrollLine, handleChannelSelect]
  );

  const handleNavBack = useCallback(async () => {
    const entry = useChatStore.getState().popNav();
    if (!entry) return;
    selectChannel(entry.channel);
    clearUnread(entry.channel);
    setMessages([]);
    setThreadRoot(null);
    const apiChannel = toApiChannel(entry.channel);
    const res = await client.read(apiChannel, 50);
    if (res.ok && res.data && useChatStore.getState().currentChannel === entry.channel) {
      setMessages(res.data.entries as Message[]);
    }
    // Restore scroll position after messages render
    requestAnimationFrame(() => {
      const el = document.querySelector("[data-message-scroll]");
      if (el) el.scrollTop = entry.scrollTop;
    });
  }, [selectChannel, clearUnread, setMessages, setThreadRoot]);

  const handleCloseUserCard = useCallback(() => {
    setUserCardHandler(null);
    setUserCardPosition(null);
  }, []);

  return (
    <div className="flex h-full overflow-hidden">
      {/* Left: sidebar */}
      <Sidebar
        onChannelSelect={handleChannelSelect}
        onStartDm={handleStartDm}
      />

      {/* Center: main content */}
      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        <ChatHeader onStartDm={handleStartDm}>
          {navHistory.length > 0 && (
            <button
              onClick={handleNavBack}
              className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors mr-2"
              title="Back"
            >
              <ArrowLeft className="h-3.5 w-3.5" />
              <span>Back</span>
            </button>
          )}
        </ChatHeader>

        {/* Message area */}
        <MessageList
          onReply={handleReply}
          onShowThread={handleShowThread}
          onMentionClick={handleMentionClick}
          onChannelClick={handleChannelClick}
          onMessageLinkClick={handleMessageLinkClick}
          onUserProfileClick={handleUserProfileClick}
        />

        <InputArea onSend={handleSend} />
      </div>

      {/* Right: thread panel */}
      <ThreadPanel
        onReplyInThread={handleReply}
        onMentionClick={handleMentionClick}
        onChannelClick={handleChannelClick}
        onMessageLinkClick={handleMessageLinkClick}
        onUserProfileClick={handleUserProfileClick}
      />

      {/* UserCard popover */}
      {userCardHandler && userCardPosition && (
        <UserCard
          handler={userCardHandler}
          position={userCardPosition}
          onClose={handleCloseUserCard}
          onStartDm={handleStartDm}
        />
      )}
    </div>
  );
}
