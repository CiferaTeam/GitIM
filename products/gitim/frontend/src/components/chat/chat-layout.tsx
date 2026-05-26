import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ArrowLeft, AtSign, Hash, LayoutGrid, LogIn, Menu } from "lucide-react";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import { useIsMobile } from "../../hooks/use-media-query";
import * as client from "../../lib/client";
import { formatDmDisplayName } from "../../lib/dm-display-name";
import { expandAllMentions } from "../../lib/expand-all-mentions";
import { buildMentionCandidates } from "../../lib/mention-candidates";
import type { Channel, Message } from "../../lib/types";
import {
  recordRemoteSyncPending,
  remoteSyncFailure,
} from "../../lib/remote-sync-toast";
import {
  chatScopeKeyForName,
  chatScopeName,
  clearChatScopeUnread,
  readActiveChatScope,
  readChatScopeState,
  readChatScopeViewAnchor,
  writeActiveChatScope,
  writeChatScopeViewAnchor,
  type ChatViewportAnchor,
} from "../../lib/chat-ui-state";
import { workspaceIdentity } from "../../lib/workspace-key";
import { Button } from "../ui/button";
import { ChannelCardDrawer } from "../cards/channel-card-drawer";
import { MobileSidebarDrawer } from "../mobile/mobile-sidebar-drawer";
import { MobileThreadOverlay } from "../mobile/mobile-thread-overlay";
import { MobileActionSheet } from "../mobile/mobile-action-sheet";
import { ChannelActiveRuns } from "../flows/channel-active-runs";
import { ChatHeader } from "./header";
import { InputArea } from "./input-area";
import { MessageList } from "./message-list";
import { ScrollToBottomButton } from "./scroll-to-bottom-button";
import { Sidebar } from "./sidebar";
import { ThreadPanel } from "./thread-panel";
import { UserCard } from "./user-card";
import {
  MESSAGES_PAGE_SIZE,
  computeAnchoredReadSince,
  computeLoadOlderSince,
} from "./pagination";
import { useScrollAtBottom } from "../../hooks/use-scroll-at-bottom";

/** "alice--lewis" → "dm:alice,lewis" */
function toApiChannel(displayName: string): string {
  if (displayName.includes("--")) {
    const parts = displayName.split("--");
    return `dm:${parts.join(",")}`;
  }
  return displayName;
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
  // Scroll container ref shared between MessageList (where it attaches) and
  // useScrollAtBottom (where the jump-to-latest button reads its state).
  const messageScrollRef = useRef<HTMLDivElement | null>(null);
  const { atBottom: messagesAtBottom, scrollToBottom: scrollMessagesToBottom } =
    useScrollAtBottom(messageScrollRef);

  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const archivedChannels = useChatStore((s) => s.archivedChannels);
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

  const currentChannelData = currentChannel
    ? channels.find((c) => c.name === currentChannel)
    : null;
  const scopeKeyForChannelName = useCallback(
    (name: string): string => {
      const channel = channels.find((c) => c.name === name);
      return chatScopeKeyForName(name, channel?.kind);
    },
    [channels],
  );
  const currentScopeKey = currentChannel
    ? scopeKeyForChannelName(currentChannel)
    : null;
  const allMentionRecipients = useMemo(
    () => currentChannelData?.kind === "channel" ? currentChannelData.members : [],
    [currentChannelData],
  );
  const mentionCandidates = useMemo(() => {
    return buildMentionCandidates({
      users,
      agents: agents.map((a) => a.id),
      includeAll: allMentionRecipients.length > 0,
    });
  }, [users, agents, allMentionRecipients]);

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
  // A selection is "archived" iff it isn't in active channels AND either
  // matches an archived channel record OR has DM-stem shape (`--`-joined,
  // and not a known archived channel name). The shape proxy is necessary
  // because the archived DMs view is now lazy-loaded and may be null when
  // the user lands on an archived DM via a stored selection. Worst case is
  // a false-positive banner over an empty timeline if the daemon can't
  // resolve the name — recoverable on the next channel poll.
  const isArchivedView =
    !!currentChannel &&
    !currentChannelData &&
    (archivedChannels.some((c) => c.name === currentChannel) ||
      currentChannel.includes("--"));
  const showJoinBanner =
    !isArchivedView &&
    !!currentChannelData &&
    currentChannelData.kind === "channel" &&
    !currentChannelData.members.includes(currentUser);

  const [userCardHandler, setUserCardHandler] = useState<string | null>(null);
  const [userCardPosition, setUserCardPosition] = useState<{ x: number; y: number } | null>(null);

  // Card drawer state — auto-close when switching channels.
  const [cardDrawerOpen, setCardDrawerOpen] = useState(false);
  // Lazy init so a tab switch (chat ↔ cards) — which unmounts/remounts this
  // route — lands back on the same scroll position instead of snapping to the
  // bottom. User-initiated channel switches still clear this via
  // setRestoreAnchor(null) inside handleChannelSelect so they show the latest.
  const [restoreAnchor, setRestoreAnchor] = useState<ChatViewportAnchor | null>(
    () => {
      const chatStateNow = useChatStore.getState();
      const channelName = chatStateNow.currentChannel;
      if (!channelName) return null;
      const channelKind = chatStateNow.channels.find(
        (c) => c.name === channelName,
      )?.kind;
      const scopeKey = chatScopeKeyForName(channelName, channelKind);
      const wsStateNow = useWorkspaceStore.getState();
      const modeNow = useConnectionStore.getState().mode;
      const active = wsStateNow.activeSlug
        ? wsStateNow.workspaces.find((w) => w.slug === wsStateNow.activeSlug)
        : null;
      if (!active) return null;
      const wsKey = workspaceIdentity(modeNow, active);
      return readChatScopeViewAnchor(wsKey, scopeKey);
    },
  );
  const viewAnchorsRef = useRef<Map<string, ChatViewportAnchor>>(new Map());
  useEffect(() => {
    // Intentional: UX contract is "switching context closes transient overlays".
    setCardDrawerOpen(false);
  }, [currentChannel]);

  const rememberCurrentScroll = useCallback(() => {
    if (!currentScopeKey) return;
    const anchor = viewAnchorsRef.current.get(currentScopeKey);
    if (anchor) {
      writeChatScopeViewAnchor(workspaceKey, currentScopeKey, anchor);
    }
  }, [currentScopeKey, workspaceKey]);

  const handleViewportAnchorChange = useCallback(
    (anchor: ChatViewportAnchor) => {
      if (!currentScopeKey) return;
      viewAnchorsRef.current.set(currentScopeKey, anchor);
      writeChatScopeViewAnchor(workspaceKey, currentScopeKey, anchor);
    },
    [currentScopeKey, workspaceKey],
  );

  const handleChannelSelect = useCallback(
    async (
      name: string,
      options: { markRead?: boolean; targetLine?: number } = {},
    ) => {
      if (!activeSlug) return;
      const requestSlug = activeSlug;
      const requestWorkspaceKey = workspaceKey;
      const targetScopeKey = scopeKeyForChannelName(name);
      const targetState = readChatScopeState(requestWorkspaceKey, targetScopeKey);
      const unreadTargetLine =
        options.markRead !== false && targetState.unreadCount > 0
          ? targetState.firstUnreadLine
          : null;
      const pendingTargetLine = options.targetLine ?? unreadTargetLine ?? null;
      rememberCurrentScroll();
      setRestoreAnchor(null);
      selectChannel(name);
      setPendingScrollLine(pendingTargetLine);
      writeActiveChatScope(requestWorkspaceKey, targetScopeKey);
      setMessages([]);
      setThreadRoot(null);
      const apiChannel = toApiChannel(name);
      const res = await client.read(
        requestSlug,
        apiChannel,
        MESSAGES_PAGE_SIZE,
        pendingTargetLine ? computeAnchoredReadSince(pendingTargetLine) : undefined,
      );
      if (
        res.ok &&
        res.data &&
        isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
        useChatStore.getState().currentChannel === name
      ) {
        setMessages(res.data.entries as Message[]);
        if (options.markRead !== false) {
          clearUnread(name);
          clearChatScopeUnread(requestWorkspaceKey, targetScopeKey);
        }
      }
    },
    [
      activeSlug,
      workspaceKey,
      rememberCurrentScroll,
      scopeKeyForChannelName,
      selectChannel,
      clearUnread,
      setMessages,
      setThreadRoot,
      setPendingScrollLine,
    ]
  );

  useEffect(() => {
    if (currentChannel) return;
    const storedName = chatScopeName(readActiveChatScope(workspaceKey));
    const stored =
      storedName && channels.some((c) => c.name === storedName)
        ? storedName
        : null;
    const fallback = stored ?? channels.find((c) => c.name === "general")?.name;
    if (fallback) {
      handleChannelSelect(fallback, { markRead: false });
    }
  }, [channels, currentChannel, handleChannelSelect, workspaceKey]);

  useEffect(() => {
    if (currentScopeKey) {
      writeActiveChatScope(workspaceKey, currentScopeKey);
    }
  }, [workspaceKey, currentScopeKey]);

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
      const expandedBody = expandAllMentions(body, allMentionRecipients, {
        referenceNonRecipients: currentChannelData?.kind === "channel",
        excludeSelf: currentUser,
      });
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
        body: expandedBody,
        _status: "sending",
        _pendingId: pendingId,
      };
      addPendingMessage(pending);

      const apiChannel = toApiChannel(requestChannel);
      const res = await client.send(
        requestSlug,
        apiChannel,
        expandedBody,
        currentUser,
        pointTo
      );
      if (!isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey)) {
        return res;
      }
      if (res.ok && res.data) {
        const lineNumber = res.data.line_number as number;
        const syncError = remoteSyncFailure(res.data);
        // commit_only means the local commit succeeded; sync_loop retries on
        // the next cycle. Treat as sent and soften the notice — the underlying
        // race (pull-only cycle mid-fetch) usually recovers within seconds.
        markPendingSent(pendingId, lineNumber);
        if (syncError) {
          recordRemoteSyncPending(
            requestWorkspaceKey,
            {
              scope: apiChannel,
              author: currentUser,
              body: expandedBody,
              lineNumber,
            },
            syncError,
          );
        }
      } else {
        markPendingFailed(pendingId);
      }
      return res;
    },
    [
      activeSlug,
      workspaceKey,
      currentChannel,
      currentUser,
      allMentionRecipients,
      currentChannelData?.kind,
      addPendingMessage,
      markPendingSent,
      markPendingFailed,
    ]
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

  const getCurrentAnchor = useCallback((): ChatViewportAnchor | null => {
    if (!currentScopeKey) return null;
    return viewAnchorsRef.current.get(currentScopeKey) ??
      readChatScopeViewAnchor(workspaceKey, currentScopeKey);
  }, [currentScopeKey, workspaceKey]);

  const handleChannelClick = useCallback(
    (channel: string) => {
      const anchor = getCurrentAnchor();
      if (currentChannel && anchor) {
        pushNav({ channel: currentChannel, anchor });
      }
      handleChannelSelect(channel);
    },
    [currentChannel, pushNav, getCurrentAnchor, handleChannelSelect]
  );

  const handleMessageLinkClick = useCallback(
    (channel: string, line: number) => {
      const anchor = getCurrentAnchor();
      if (currentChannel && anchor) {
        pushNav({ channel: currentChannel, anchor });
      }
      handleChannelSelect(channel, { targetLine: line });
    },
    [currentChannel, pushNav, getCurrentAnchor, handleChannelSelect]
  );

  const handleNavBack = useCallback(async () => {
    const entry = useChatStore.getState().popNav();
    if (!entry || !activeSlug) return;
    const requestSlug = activeSlug;
    const requestWorkspaceKey = workspaceKey;
    const entryScopeKey = scopeKeyForChannelName(entry.channel);
    rememberCurrentScroll();
    setRestoreAnchor(entry.anchor);
    setPendingScrollLine(null);
    selectChannel(entry.channel);
    writeActiveChatScope(requestWorkspaceKey, entryScopeKey);
    setMessages([]);
    setThreadRoot(null);
    const apiChannel = toApiChannel(entry.channel);
    const res = await client.read(
      requestSlug,
      apiChannel,
      MESSAGES_PAGE_SIZE,
      computeAnchoredReadSince(entry.anchor.line),
    );
    if (
      res.ok &&
      res.data &&
      isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey) &&
      useChatStore.getState().currentChannel === entry.channel
    ) {
      setMessages(res.data.entries as Message[]);
      clearUnread(entry.channel);
      clearChatScopeUnread(requestWorkspaceKey, entryScopeKey);
    }
  }, [
    activeSlug,
    workspaceKey,
    rememberCurrentScroll,
    scopeKeyForChannelName,
    selectChannel,
    clearUnread,
    setMessages,
    setThreadRoot,
    setPendingScrollLine,
  ]);

  // In-flight guard for history paging. Plain ref (not store state) — the
  // scroll handler may fire many times during a fast scroll, and we drop
  // calls while a fetch is already pending. Doesn't need re-render triggers.
  //
  // Reset on channel / workspace switch so a fetch that's still in flight
  // for the previous context doesn't silently swallow the user's first
  // scroll-to-top in the new context. The previous-context response is also
  // dropped by the stale-check inside handleLoadOlder; this effect just
  // ensures the new context's scroll handler can fire immediately.
  const loadingOlderRef = useRef(false);
  useEffect(() => {
    loadingOlderRef.current = false;
  }, [currentChannel, workspaceKey]);

  const handleLoadOlder = useCallback(async () => {
    if (loadingOlderRef.current) return;
    if (!activeSlug || !currentChannel) return;

    const snapshot = useChatStore.getState();
    if (!snapshot.hasMoreHistory) return;

    // Oldest real (non-pending) message currently on screen.
    const oldestLine = snapshot.messages.find((m) => !m._pendingId)?.line_number;
    const decision = computeLoadOlderSince(oldestLine, MESSAGES_PAGE_SIZE);
    if (decision.kind === "skip") {
      if (decision.reason === "at_top") {
        snapshot.setHasMoreHistory(false);
      }
      return;
    }

    loadingOlderRef.current = true;
    const requestSlug = activeSlug;
    const requestWorkspaceKey = workspaceKey;
    const requestChannel = currentChannel;
    const apiChannel = toApiChannel(requestChannel);
    try {
      const res = await client.read(
        requestSlug,
        apiChannel,
        MESSAGES_PAGE_SIZE,
        decision.since,
      );
      // Workspace switched or channel switched while in flight — drop.
      if (!isCurrentWorkspaceRequest(requestSlug, requestWorkspaceKey)) return;
      if (useChatStore.getState().currentChannel !== requestChannel) return;

      if (!res.ok || !res.data) {
        // Silent transient failure. Logging only — toast would be noisy for
        // a background scroll trigger; the next scroll naturally retries.
        console.warn("Failed to load older messages:", res.error);
        return;
      }

      const olderEntries = (res.data.entries ?? []) as Message[];
      if (olderEntries.length > 0) {
        useChatStore.getState().prependMessages(olderEntries);
      }
      // Short response = no more history beyond what we just got.
      if (olderEntries.length < MESSAGES_PAGE_SIZE) {
        useChatStore.getState().setHasMoreHistory(false);
      }
    } finally {
      loadingOlderRef.current = false;
    }
  }, [activeSlug, workspaceKey, currentChannel]);

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
      ? formatDmDisplayName(currentChannel, currentUser)
      : currentChannel
    : "Select a channel";

  return (
    <div className="flex h-full overflow-hidden">
      {/* Desktop sidebar */}
      <div className="hidden md:block h-full">
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
                // When `isArchivedView` is true the current selection isn't in
                // active channels. It's a DM iff its name is the sorted-pair
                // stem (`<min>--<max>`) and not a known archived channel — the
                // shape check works whether the view has been expanded yet or
                // not, so this banner stays correct for deep-linked archived
                // DMs.
                const isDm =
                  !!currentChannel &&
                  currentChannel.includes("--") &&
                  !archivedChannels.some((c) => c.name === currentChannel);
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

        {currentChannel && currentChannelData?.kind === "channel" && (
          <ChannelActiveRuns channel={currentChannel} />
        )}
        <MessageList
          messages={messages}
          currentUser={currentUser}
          scopeKey={currentChannel}
          replyTo={replyTo}
          highlightLine={highlightLine}
          pendingScrollLine={pendingScrollLine}
          restoreAnchor={restoreAnchor}
          onHighlightLineChange={setHighlightLine}
          onPendingScrollClear={handlePendingScrollClear}
          onViewportAnchorChange={handleViewportAnchorChange}
          onReply={handleReply}
          onShowThread={handleShowThread}
          onMentionClick={handleMentionClick}
          onChannelClick={handleChannelClick}
          onMessageLinkClick={handleMessageLinkClick}
          onUserProfileClick={handleUserProfileClick}
          onActionSheet={isMobile ? setActionSheetMessage : undefined}
          onLoadOlder={handleLoadOlder}
          scrollRef={messageScrollRef}
        />
        {currentChannel && currentChannelData && !isArchivedView && (
          <div className="relative shrink-0">
            <ScrollToBottomButton
              visible={!messagesAtBottom}
              onClick={() => scrollMessagesToBottom()}
            />
            <InputArea
              workspaceKey={workspaceKey}
              scopeKey={currentChannel}
              replyTo={replyTo}
              onReplyToChange={setReplyTo}
              mentionCandidates={mentionCandidates}
              recipientChannel={currentChannelData}
              messages={messages}
              currentUser={currentUser}
              disabled={isGuest}
              onSend={handleSend}
            />
          </div>
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
