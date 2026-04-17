import { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowLeft, LogIn } from "lucide-react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import * as client from "../../lib/client";
import type { Channel, Message } from "../../lib/types";
import { Button } from "../ui/button";
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
  const isGuest = useChatStore((s) => s.isGuest);
  const users = useChatStore((s) => s.users);
  const messages = useChatStore((s) => s.messages);
  const replyTo = useChatStore((s) => s.replyTo);
  const highlightLine = useChatStore((s) => s.highlightLine);
  const pendingScrollLine = useChatStore((s) => s.pendingScrollLine);
  const threadRoot = useChatStore((s) => s.threadRoot);
  const threadMessages = useChatStore((s) => s.threadMessages);
  const agents = useAgentStore((s) => s.agents);

  const selectChannel = useChatStore((s) => s.selectChannel);
  const clearUnread = useChatStore((s) => s.clearUnread);
  const setMessages = useChatStore((s) => s.setMessages);
  const addPendingMessage = useChatStore((s) => s.addPendingMessage);
  const markPendingSent = useChatStore((s) => s.markPendingSent);
  const markPendingFailed = useChatStore((s) => s.markPendingFailed);
  const setReplyTo = useChatStore((s) => s.setReplyTo);
  const setHighlightLine = useChatStore((s) => s.setHighlightLine);
  const setPendingScrollLine = useChatStore((s) => s.setPendingScrollLine);
  const setThreadRoot = useChatStore((s) => s.setThreadRoot);
  const setThreadMessages = useChatStore((s) => s.setThreadMessages);
  const setChannels = useChatStore((s) => s.setChannels);
  const pushNav = useChatStore((s) => s.pushNav);
  const navHistory = useChatStore((s) => s.navHistory);

  const mentionCandidates = useMemo(() => {
    const agentIds = agents.map((a) => a.id);
    const set = new Set([...users, ...agentIds]);
    return [...set];
  }, [users, agents]);

  const currentChannelData = currentChannel
    ? channels.find((c) => c.name === currentChannel)
    : null;
  const showJoinBanner =
    !!currentChannelData &&
    currentChannelData.kind === "channel" &&
    !currentChannelData.members.includes(currentUser);

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
      if (res.ok && res.data && useChatStore.getState().currentChannel === name) {
        setMessages(res.data.entries as Message[]);
      }
    },
    [selectChannel, clearUnread, setMessages, setThreadRoot]
  );

  useEffect(() => {
    if (currentChannel) return;
    const general = channels.find((c) => c.name === "general");
    if (general) {
      handleChannelSelect("general");
    }
  }, [channels, currentChannel, handleChannelSelect]);

  const handleJoin = useCallback(async () => {
    if (!currentChannel) return;
    const res = await client.joinChannel(currentChannel);
    if (!res.ok) return;
    const chRes = await client.channels();
    if (chRes.ok && chRes.data) {
      setChannels(chRes.data.channels as Channel[]);
    }
    const apiChannel = toApiChannel(currentChannel);
    const readRes = await client.read(apiChannel, 50);
    if (readRes.ok && readRes.data) {
      setMessages(readRes.data.entries as Message[]);
    }
  }, [currentChannel, setChannels, setMessages]);

  const handleStartDm = useCallback(
    async (targetUser: string) => {
      const parts = [currentUser, targetUser].sort();
      const displayName = parts.join("--");
      const exists = channels.some((c) => c.name === displayName);
      if (!exists) {
        const newChannel = { name: displayName, kind: "dm" as const, unreadCount: 0, hasMention: false, members: parts };
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
      if (currentChannel) {
        pushNav({ channel: currentChannel, scrollTop: getScrollTop() });
      }
      handleChannelSelect(channel);
    },
    [currentChannel, pushNav, getScrollTop, handleChannelSelect]
  );

  const handleMessageLinkClick = useCallback(
    (channel: string, line: number) => {
      if (currentChannel) {
        pushNav({ channel: currentChannel, scrollTop: getScrollTop() });
      }
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
    requestAnimationFrame(() => {
      const el = document.querySelector("[data-message-scroll]");
      if (el) el.scrollTop = entry.scrollTop;
    });
  }, [selectChannel, clearUnread, setMessages, setThreadRoot]);

  const handleCloseUserCard = useCallback(() => {
    setUserCardHandler(null);
    setUserCardPosition(null);
  }, []);

  const handlePendingScrollClear = useCallback(() => {
    setPendingScrollLine(null);
  }, [setPendingScrollLine]);

  const handleThreadClose = useCallback(() => {
    setThreadRoot(null);
  }, [setThreadRoot]);

  return (
    <div className="flex h-full overflow-hidden">
      <Sidebar
        onChannelSelect={handleChannelSelect}
        onStartDm={handleStartDm}
      />

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

        {showJoinBanner && (
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60 bg-muted/50">
            <span className="text-xs text-muted-foreground">
              You're viewing #{currentChannel} but haven't joined
            </span>
            <Button onClick={handleJoin} variant="outline" size="xs" className="gap-1.5">
              <LogIn className="size-3" />
              Join
            </Button>
          </div>
        )}

        <MessageList
          messages={messages}
          scopeKey={currentChannel}
          replyTo={replyTo}
          highlightLine={highlightLine}
          pendingScrollLine={pendingScrollLine}
          onHighlightLineChange={setHighlightLine}
          onPendingScrollClear={handlePendingScrollClear}
          onReply={handleReply}
          onShowThread={handleShowThread}
          onMentionClick={handleMentionClick}
          onChannelClick={handleChannelClick}
          onMessageLinkClick={handleMessageLinkClick}
          onUserProfileClick={handleUserProfileClick}
        />
        {currentChannel && (
          <InputArea
            scopeKey={currentChannel}
            replyTo={replyTo}
            onReplyToChange={setReplyTo}
            mentionCandidates={mentionCandidates}
            disabled={isGuest}
            onSend={handleSend}
          />
        )}
      </div>

      <ThreadPanel
        root={threadRoot}
        messages={threadMessages}
        onClose={handleThreadClose}
        onReplyInThread={handleReply}
        onMentionClick={handleMentionClick}
        onChannelClick={handleChannelClick}
        onMessageLinkClick={handleMessageLinkClick}
        onUserProfileClick={handleUserProfileClick}
      />

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
