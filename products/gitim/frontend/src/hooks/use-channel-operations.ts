import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as client from "../lib/client";
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
} from "../lib/chat-ui-state";
import { expandAllMentions } from "../lib/expand-all-mentions";
import {
  recordRemoteSyncPending,
  remoteSyncFailure,
} from "../lib/remote-sync-toast";
import { toApiChannel } from "../lib/scope-name";
import type { ApiResponse, Channel, Message } from "../lib/types";
import { workspaceIdentity } from "../lib/workspace-key";
import {
  MESSAGES_PAGE_SIZE,
  computeAnchoredReadSince,
  computeLoadOlderSince,
} from "../components/chat/pagination";
import { useChatStore } from "./use-chat-store";
import { useConnectionStore } from "./use-connection-store";
import { useWorkspaceStore } from "./use-workspace-store";

/**
 * Owns the channel-business-logic surface area that ChatLayout used to carry
 * inline: channel selection, join, send, thread, link-jump, nav back,
 * pagination, plus the viewport-anchor cache that ties scroll restoration to
 * those operations.
 *
 * The hook reads workspace / chat state via Zustand selectors so its callers
 * don't need to thread inputs through — the same selectors ChatLayout would
 * have subscribed to anyway. Trivial UI handlers (replyTo wrapper, UserCard
 * popover state, mobile bottom-sheet toggles) stay in ChatLayout because
 * they're tied to component-local transient state.
 *
 * Returns a `restoreAnchor` value alongside the handlers so MessageList can
 * still restore scroll position on tab/route remount — the anchor cache and
 * the operations that mutate it (handleChannelSelect → clear; handleNavBack
 * → set) live together so they can't drift.
 */
export interface ChannelOperations {
  restoreAnchor: ChatViewportAnchor | null;
  handleChannelSelect: (
    name: string,
    options?: { markRead?: boolean; targetLine?: number },
  ) => Promise<void>;
  handleJoin: () => Promise<void>;
  handleStartDm: (targetUser: string) => Promise<void>;
  handleSend: (body: string, pointTo?: number) => Promise<ApiResponse>;
  handleShowThread: (msg: Message) => Promise<void>;
  handleChannelClick: (channel: string) => void;
  handleMessageLinkClick: (channel: string, line: number) => void;
  handleNavBack: () => Promise<void>;
  handleLoadOlder: () => Promise<void>;
  handleViewportAnchorChange: (anchor: ChatViewportAnchor) => void;
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
  return (
    useWorkspaceStore.getState().activeSlug === slug &&
    currentWorkspaceKey() === key
  );
}

export function useChannelOperations(): ChannelOperations {
  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const currentUser = useChatStore((s) => s.currentUser);
  const selectChannel = useChatStore((s) => s.selectChannel);
  const clearUnread = useChatStore((s) => s.clearUnread);
  const setMessages = useChatStore((s) => s.setMessages);
  const addPendingMessage = useChatStore((s) => s.addPendingMessage);
  const markPendingSent = useChatStore((s) => s.markPendingSent);
  const markPendingFailed = useChatStore((s) => s.markPendingFailed);
  const setPendingScrollLine = useChatStore((s) => s.setPendingScrollLine);
  const setThreadRoot = useChatStore((s) => s.setThreadRoot);
  const setThreadMessages = useChatStore((s) => s.setThreadMessages);
  const setChannels = useChatStore((s) => s.setChannels);
  const pushNav = useChatStore((s) => s.pushNav);

  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;

  const currentChannelData = currentChannel
    ? channels.find((c) => c.name === currentChannel) ?? null
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
    () =>
      currentChannelData?.kind === "channel" ? currentChannelData.members : [],
    [currentChannelData],
  );

  // Lazy init so tab switches (chat ↔ cards) — which unmount/remount
  // ChatLayout (and therefore this hook) — land back on the same scroll
  // position instead of snapping to the bottom. User-initiated channel
  // switches still clear this via setRestoreAnchor(null) inside
  // handleChannelSelect so they show the latest.
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
      const targetState = readChatScopeState(
        requestWorkspaceKey,
        targetScopeKey,
      );
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
        pendingTargetLine
          ? computeAnchoredReadSince(pendingTargetLine)
          : undefined,
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
    ],
  );

  // Fallback channel selection on first mount: if no channel is current,
  // pick the stored one (or "general") so the chat view always has a scope.
  useEffect(() => {
    if (currentChannel) return;
    const storedName = chatScopeName(readActiveChatScope(workspaceKey));
    const stored =
      storedName && channels.some((c) => c.name === storedName)
        ? storedName
        : null;
    const fallback =
      stored ?? channels.find((c) => c.name === "general")?.name;
    if (fallback) {
      void handleChannelSelect(fallback, { markRead: false });
    }
  }, [channels, currentChannel, handleChannelSelect, workspaceKey]);

  // Mirror the active scope into storage so it survives reload.
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
    const readRes = await client.read(
      requestSlug,
      apiChannel,
      MESSAGES_PAGE_SIZE,
    );
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
        const newChannel: Channel = {
          name: displayName,
          kind: "dm",
          unreadCount: 0,
          hasMention: false,
          members: parts,
        };
        setChannels([...channels, newChannel]);
      }
      await handleChannelSelect(displayName);
    },
    [currentUser, channels, setChannels, handleChannelSelect],
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
        pointTo,
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
    ],
  );

  const handleShowThread = useCallback(
    async (msg: Message) => {
      if (!currentChannel || !activeSlug) return;
      const requestSlug = activeSlug;
      const requestWorkspaceKey = workspaceKey;
      const requestChannel = currentChannel;
      const apiChannel = toApiChannel(requestChannel);
      const res = await client.thread(
        requestSlug,
        apiChannel,
        msg.line_number,
      );
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
    [
      activeSlug,
      workspaceKey,
      currentChannel,
      setThreadRoot,
      setThreadMessages,
    ],
  );

  const getCurrentAnchor = useCallback((): ChatViewportAnchor | null => {
    if (!currentScopeKey) return null;
    return (
      viewAnchorsRef.current.get(currentScopeKey) ??
      readChatScopeViewAnchor(workspaceKey, currentScopeKey)
    );
  }, [currentScopeKey, workspaceKey]);

  const handleChannelClick = useCallback(
    (channel: string) => {
      const anchor = getCurrentAnchor();
      if (currentChannel && anchor) {
        pushNav({ channel: currentChannel, anchor });
      }
      void handleChannelSelect(channel);
    },
    [currentChannel, pushNav, getCurrentAnchor, handleChannelSelect],
  );

  const handleMessageLinkClick = useCallback(
    (channel: string, line: number) => {
      const anchor = getCurrentAnchor();
      if (currentChannel && anchor) {
        pushNav({ channel: currentChannel, anchor });
      }
      void handleChannelSelect(channel, { targetLine: line });
    },
    [currentChannel, pushNav, getCurrentAnchor, handleChannelSelect],
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
  // Reset on channel / workspace switch so a fetch that's still in flight for
  // the previous context doesn't silently swallow the user's first
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
    const oldestLine = snapshot.messages.find(
      (m) => !m._pendingId,
    )?.line_number;
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
        // Silent transient failure. Logging only — toast would be noisy for a
        // background scroll trigger; the next scroll naturally retries.
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

  return {
    restoreAnchor,
    handleChannelSelect,
    handleJoin,
    handleStartDm,
    handleSend,
    handleShowThread,
    handleChannelClick,
    handleMessageLinkClick,
    handleNavBack,
    handleLoadOlder,
    handleViewportAnchorChange,
  };
}
