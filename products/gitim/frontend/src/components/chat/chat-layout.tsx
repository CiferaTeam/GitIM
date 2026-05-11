import { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowLeft, AtSign, Hash, LayoutGrid, LogIn, Menu } from "lucide-react";
import { toast } from "sonner";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { useIsMobile } from "../../hooks/use-media-query";
import * as client from "../../lib/client";
import type { Channel, Message } from "../../lib/types";
import { workspaceIdentity } from "../../lib/workspace-key";
import { Button } from "../ui/button";
import { ChannelCardDrawer } from "../cards/channel-card-drawer";
import { MobileSidebarDrawer } from "../mobile/mobile-sidebar-drawer";
import { MobileThreadOverlay } from "../mobile/mobile-thread-overlay";
import { MobileActionSheet } from "../mobile/mobile-action-sheet";
import { ChatHeader } from "./header";
import { InputArea } from "./input-area";
import { MessageList } from "./message-list";
import { Sidebar } from "./sidebar";
import { ThreadPanel } from "./thread-panel";
import { UserCard } from "./user-card";
import { MESSAGES_PAGE_SIZE } from "./pagination";

/** "alice--lewis" → "dm:alice,lewis" */
function toApiChannel(displayName: string): string {
  if (displayName.includes("--")) {
    const parts = displayName.split("--");
    return `dm:${parts.join(",")}`;
  }
  return displayName;
}

function syncFailure(data: Record<string, unknown> | undefined): string | null {
  if (!data) return null;
  const status = typeof data.status === "string" ? data.status : data.sync_status;
  const error = typeof data.error === "string"
    ? data.error
    : typeof data.sync_error === "string"
      ? data.sync_error
      : null;
  return status === "commit_only" || error ? error ?? "Sync failed" : null;
}

function currentWorkspaceKey(): string | null {
  const mode = useConnectionStore.getState().mode;
  const { activeSlug, workspaces } = useWorkspaceStore.getState();
  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  return activeWorkspace ? workspaceIdentity(mode, activeWorkspace) : null;
}

function isCurrentWorkspaceRequest(slug: string, key: string | null): boolean {
  return useWorkspaceStore.getState().activeSlug === slug &&
    currentWorkspaceKey() === key;
}

export function ChatLayout() {
  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const archivedChannels = useChatStore((s) => s.archivedChannels);
  const archivedDms = useChatStore((s) => s.archivedDms);
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
  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;
  // An archived channel/DM is one the user opens from an Archived section —
  // it never shows up in the active `channels` list. Read-only view: message
  // fetch already works (daemon's read handler falls back to archive/
  // automatically), but writes must be blocked.
  const isArchivedView =
    !!currentChannel &&
    !currentChannelData &&
    (archivedChannels.some((c) => c.name === currentChannel) ||
      archivedDms.some((c) => c.name === currentChannel));
  const showJoinBanner =
    !isArchivedView &&
    !!currentChannelData &&
    currentChannelData.kind === "channel" &&
    !currentChannelData.members.includes(currentUser);

  const [userCardHandler, setUserCardHandler] = useState<string | null>(null);
  const [userCardPosition, setUserCardPosition] = useState<{ x: number; y: number } | null>(null);

  // Card drawer state — auto-close when switching channels.
  const [cardDrawerOpen, setCardDrawerOpen] = useState(false);
  useEffect(() => {
    // Intentional: UX contract is "switching context closes transient overlays".
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setCardDrawerOpen(false);
  }, [currentChannel]);

  const handleChannelSelect = useCallback(
    async (name: string) => {
      if (!activeSlug) return;
      const requestSlug = activeSlug;
      const requestWorkspaceKey = workspaceKey;
      selectChannel(name);
      clearUnread(name);
      setMessages([]);
      setThreadRoot(null);
      const apiChannel = toApiChannel(name);
      const res = await client.read(requestSlug, apiChannel, MESSAGES_PAGE_SIZE);
      if (
        res.ok &&
        res.data &&
        isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
        useChatStore.getState().currentChannel === name
      ) {
        setMessages(res.data.entries as Message[]);
      }
    },
    [activeSlug, workspaceKey, selectChannel, clearUnread, setMessages, setThreadRoot]
  );

  useEffect(() => {
    if (currentChannel) return;
    const general = channels.find((c) => c.name === "general");
    if (general) {
      handleChannelSelect("general");
    }
  }, [channels, currentChannel, handleChannelSelect]);

  const handleJoin = useCallback(async () => {
    if (!currentChannel || !activeSlug) return;
    const requestSlug = activeSlug;
    const requestWorkspaceKey = workspaceKey;
    const requestChannel = currentChannel;
    const res = await client.joinChannel(requestSlug, requestChannel);
    if (!isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey)) return;
    if (!res.ok) return;
    const chRes = await client.channels(requestSlug);
    if (!isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey)) return;
    if (chRes.ok && chRes.data) {
      setChannels(chRes.data.channels as Channel[]);
    }
    const apiChannel = toApiChannel(requestChannel);
    const readRes = await client.read(requestSlug, apiChannel, MESSAGES_PAGE_SIZE);
    if (
      readRes.ok &&
      readRes.data &&
      isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
      useChatStore.getState().currentChannel === requestChannel
    ) {
      setMessages(readRes.data.entries as Message[]);
    }
  }, [activeSlug, workspaceKey, currentChannel, setChannels, setMessages]);

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
      if (!activeSlug) return { ok: false, error: "No workspace selected" };
      const requestSlug = activeSlug;
      const requestWorkspaceKey = workspaceKey;
      const requestChannel = currentChannel;
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

      const apiChannel = toApiChannel(requestChannel);
      const res = await client.send(
        requestSlug,
        apiChannel,
        body,
        currentUser,
        pointTo
      );
      if (!isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey)) {
        return res;
      }
      if (res.ok && res.data) {
        const lineNumber = res.data.line_number as number;
        const syncError = syncFailure(res.data);
        if (syncError) {
          markPendingFailed(pendingId, lineNumber);
          toast.error(`Message saved locally, sync failed: ${syncError}`);
        } else {
          markPendingSent(pendingId, lineNumber);
        }
      } else {
        markPendingFailed(pendingId);
      }
      return res;
    },
    [activeSlug, workspaceKey, currentChannel, currentUser, addPendingMessage, markPendingSent, markPendingFailed]
  );

  const handleReply = useCallback(
    (msg: Message) => {
      setReplyTo(msg);
    },
    [setReplyTo]
  );

  const handleShowThread = useCallback(
    async (msg: Message) => {
      if (!currentChannel || !activeSlug) return;
      const requestSlug = activeSlug;
      const requestWorkspaceKey = workspaceKey;
      const requestChannel = currentChannel;
      const apiChannel = toApiChannel(requestChannel);
      const res = await client.thread(requestSlug, apiChannel, msg.line_number);
      if (
        res.ok &&
        res.data &&
        isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
        useChatStore.getState().currentChannel === requestChannel
      ) {
        const entries = res.data.entries as Message[];
        const root = entries[0] ?? msg;
        setThreadRoot(root);
        setThreadMessages(entries);
      }
    },
    [activeSlug, workspaceKey, currentChannel, setThreadRoot, setThreadMessages]
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
    if (!entry || !activeSlug) return;
    const requestSlug = activeSlug;
    const requestWorkspaceKey = workspaceKey;
    selectChannel(entry.channel);
    clearUnread(entry.channel);
    setMessages([]);
    setThreadRoot(null);
    const apiChannel = toApiChannel(entry.channel);
    const res = await client.read(requestSlug, apiChannel, MESSAGES_PAGE_SIZE);
    if (
      res.ok &&
      res.data &&
      isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
      useChatStore.getState().currentChannel === entry.channel
    ) {
      setMessages(res.data.entries as Message[]);
    }
    requestAnimationFrame(() => {
      if (
        !isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) ||
        useChatStore.getState().currentChannel !== entry.channel
      ) {
        return;
      }
      const el = document.querySelector("[data-message-scroll]");
      if (el) el.scrollTop = entry.scrollTop;
    });
  }, [activeSlug, workspaceKey, selectChannel, clearUnread, setMessages, setThreadRoot]);

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

  // Mobile states
  const isMobile = useIsMobile();
  const [mobileSidebarOpen, setMobileSidebarOpen] = useState(false);
  const [actionSheetMessage, setActionSheetMessage] = useState<Message | null>(null);

  const handleMobileReply = useCallback((msg: Message) => {
    setReplyTo(msg);
    setActionSheetMessage(null);
  }, [setReplyTo]);

  const handleMobileShowThread = useCallback((msg: Message) => {
    setActionSheetMessage(null);
    void handleShowThread(msg);
  }, [handleShowThread]);

  const isDm = currentChannelData?.kind === "dm";
  const mobileChannelLabel = currentChannel
    ? isDm
      ? `@${currentChannel.split("--").find((p) => p !== currentUser) ?? currentChannel}`
      : `#${currentChannel}`
    : "Select a channel";

  return (
    <div className="flex h-full overflow-hidden">
      {/* Desktop sidebar */}
      <div className="hidden md:block">
        <Sidebar
          onChannelSelect={handleChannelSelect}
          onStartDm={handleStartDm}
        />
      </div>

      <div className="flex-1 flex flex-col min-w-0 overflow-hidden">
        {/* Mobile top bar overlay when in chat */}
        {isMobile && (
          <div className="h-[52px] border-b border-border flex items-center px-3 justify-between shrink-0 bg-card/80 backdrop-blur-md md:hidden">
            <div className="flex items-center gap-2 min-w-0 flex-1">
              <button
                onClick={() => setMobileSidebarOpen(true)}
                className="p-2 -ml-1 rounded-lg hover:bg-surface transition-colors active:scale-90"
                aria-label="Open conversations"
              >
                <Menu className="size-5 text-text-muted" />
              </button>
              {navHistory.length > 0 && (
                <button
                  onClick={handleNavBack}
                  className="p-2 rounded-lg text-muted-foreground hover:text-foreground hover:bg-muted transition-colors active:scale-90"
                  aria-label="Back"
                >
                  <ArrowLeft className="h-4 w-4" />
                </button>
              )}
              <div className="flex items-center gap-1.5 min-w-0">
                {isDm ? (
                  <AtSign className="size-4 text-primary shrink-0" />
                ) : (
                  <Hash className="size-4 text-primary shrink-0" />
                )}
                <span className="font-semibold text-sm tracking-tight truncate">
                  {mobileChannelLabel}
                </span>
              </div>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              {replyTo && (
                <span className="text-[11px] text-primary bg-primary/10 px-2 py-1 rounded-full">
                  Replying
                </span>
              )}
              {currentChannel && currentChannelData?.kind === "channel" && (
                <button
                  onClick={() => setCardDrawerOpen(true)}
                  className="flex items-center gap-1 px-2 py-1.5 rounded-lg text-text-muted hover:text-foreground hover:bg-surface transition-colors active:scale-90"
                  aria-label={`Open cards for ${currentChannel}`}
                >
                  <LayoutGrid className="size-4" />
                  <span className="text-xs font-medium">Cards</span>
                </button>
              )}
            </div>
          </div>
        )}

        {/* Desktop header */}
        {!isMobile && (
          <ChatHeader
            onStartDm={handleStartDm}
            onOpenCards={
              currentChannel && currentChannelData?.kind === "channel"
                ? () => setCardDrawerOpen(true)
                : undefined
            }
          >
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
        )}

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

        {isArchivedView && (
          <div className="flex items-center justify-between px-4 py-2 border-b border-border/60 bg-muted/50">
            <span className="text-xs text-muted-foreground">
              {(() => {
                const isDm =
                  !!currentChannel &&
                  archivedDms.some((c) => c.name === currentChannel);
                if (isDm && currentChannel) {
                  const peer =
                    currentChannel
                      .split("--")
                      .find((p) => p !== currentUser) ?? currentChannel;
                  return `DM with @${peer} is archived — read only. Unarchive from the sidebar to resume the conversation.`;
                }
                return `#${currentChannel} is archived — read only. Unarchive from the sidebar to resume the conversation.`;
              })()}
            </span>
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
          onActionSheet={isMobile ? setActionSheetMessage : undefined}
        />
        {currentChannel && !isArchivedView && (
          <InputArea
            workspaceKey={workspaceKey}
            scopeKey={currentChannel}
            replyTo={replyTo}
            onReplyToChange={setReplyTo}
            mentionCandidates={mentionCandidates}
            disabled={isGuest}
            onSend={handleSend}
          />
        )}
      </div>

      {/* Desktop thread panel */}
      <div className="hidden md:block">
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
      </div>

      {/* Mobile overlays */}
      <MobileSidebarDrawer
        open={mobileSidebarOpen}
        onClose={() => setMobileSidebarOpen(false)}
        onChannelSelect={handleChannelSelect}
      />
      <MobileThreadOverlay
        root={threadRoot}
        messages={threadMessages}
        onClose={handleThreadClose}
        onReplyInThread={handleReply}
      />
      <MobileActionSheet
        message={actionSheetMessage}
        onClose={() => setActionSheetMessage(null)}
        onReply={handleMobileReply}
        onShowThread={handleMobileShowThread}
      />

      {userCardHandler && userCardPosition && (
        <UserCard
          handler={userCardHandler}
          position={userCardPosition}
          onClose={handleCloseUserCard}
          onStartDm={handleStartDm}
        />
      )}

      {currentChannel && currentChannelData?.kind === "channel" && (
        <ChannelCardDrawer
          channel={currentChannel}
          open={cardDrawerOpen}
          onOpenChange={setCardDrawerOpen}
        />
      )}
    </div>
  );
}
