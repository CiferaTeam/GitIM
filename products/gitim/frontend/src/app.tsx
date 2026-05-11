import { useCallback, useEffect, useRef } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router";
import { Loader2 } from "lucide-react";
import { BoardsView } from "./components/boards/boards-view";
import { CardDetail } from "./components/cards/card-detail";
import { CardKanban } from "./components/cards/card-kanban";
import { ChatLayout } from "./components/chat/chat-layout";
import { CronCalendar } from "./components/crons/cron-calendar";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { DocsPage } from "./components/docs/docs-page";
import { useAgentActivitySSE } from "./hooks/use-agent-activity";
import { useAgentStore } from "./hooks/use-agent-store";
import { useBoardStore } from "./hooks/use-board-store";
import { useCardStore, parseCardScope, cardPathKey } from "./hooks/use-card-store";
import { useChatStore } from "./hooks/use-chat-store";
import { useConnectionStore } from "./hooks/use-connection-store";
import { useIsMobile } from "./hooks/use-media-query";
import { useWorkspaceStore } from "./hooks/use-workspace-store";
import type {
  Agent,
  BoardSummary,
  Card,
  Channel,
  Message,
  PollChange,
} from "./lib/types";
import * as client from "./lib/client";
import { loadCursor, saveCursor, clearCursor } from "./lib/cursor";
import { readUiState } from "./lib/ui-state";
import { workspaceIdentity } from "./lib/workspace-key";
import { SetupGate } from "./components/setup/setup-gate";
import { CreateWorkspaceForm } from "./components/workspace/create-workspace-form";
import { Toaster } from "sonner";

const POLL_INTERVAL_MS = 3000;
const LOCAL_POLL_INTERVAL_MS = 7000;

// Consecutive connectivity failures (fetch-level) before we flip the
// header dot red. At 3s cadence, 3 fails ≈ 9s of unreachability.
const FAILS_UNTIL_DISCONNECTED = 3;

// After this many consecutive fails, demote connection status back to
// "disconnected" so SetupGate re-renders ConnectForm and the user gets
// an actionable reconnect path. At 3s cadence, 10 fails ≈ 30s — enough
// room for a quick runtime restart before we kick the user out.
const FAILS_UNTIL_STATUS_DEMOTE = 10;

/** "dm:alice,lewis" -> "alice--lewis"; passthrough for channels */
function apiToDisplay(channel: string): string {
  if (channel.startsWith("dm:")) {
    return channel.slice(3).replace(",", "--");
  }
  return channel;
}

/** "alice--lewis" -> "dm:alice,lewis"; passthrough for channels */
function toApiChannel(displayName: string): string {
  if (displayName.includes("--")) {
    return `dm:${displayName.split("--").join(",")}`;
  }
  return displayName;
}

function decodePathSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

function parseCardRoute(pathname: string): {
  channel: string;
  cardId: string;
} | null {
  const match = /^\/cards\/([^/]+)\/([^/]+)\/?$/.exec(pathname);
  if (!match) return null;
  return {
    channel: decodePathSegment(match[1]),
    cardId: decodePathSegment(match[2]),
  };
}

function isUnknownWorkspaceResponse(res: { ok: boolean; error?: string | null }) {
  return !res.ok && res.error === "unknown workspace";
}

function ManagementPage() {
  return <AgentList />;
}

function ChatPage() {
  return <ChatLayout />;
}

function FirstRunScreen() {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-6">
      <div className="w-full max-w-md">
        <div className="text-center mb-6">
          <h1 className="text-2xl font-bold tracking-tight">
            gitim
          </h1>
          <p className="text-sm text-text-muted mt-1">
            Create your first workspace to get started.
          </p>
        </div>
        <div className="rounded-2xl border border-border bg-card/90 shadow-xl shadow-[var(--color-shadow)] p-6">
          <CreateWorkspaceForm fullWidth />
        </div>
      </div>
    </div>
  );
}

function WorkspaceLoading() {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background text-sm text-text-muted gap-2">
      <Loader2 className="size-4 animate-spin" />
      Loading workspaces...
    </div>
  );
}

function WorkspaceIncomplete({ slug }: { slug: string }) {
  return (
    <div className="min-h-screen flex items-center justify-center bg-background p-6">
      <div className="max-w-md text-center space-y-3">
        <p className="text-base font-semibold">Workspace incomplete</p>
        <p className="text-sm text-text-muted">
          Workspace <code className="font-mono">{slug}</code> is registered but
          not fully initialized. Try creating it again, or delete it from the
          switcher and start over.
        </p>
      </div>
    </div>
  );
}

export default function App() {
  const setCurrentUser = useChatStore((s) => s.setCurrentUser);
  const setChannels = useChatStore((s) => s.setChannels);
  const setArchivedChannels = useChatStore((s) => s.setArchivedChannels);
  const setUsers = useChatStore((s) => s.setUsers);
  const setConnected = useChatStore((s) => s.setConnected);
  const addMessages = useChatStore((s) => s.addMessages);
  const setMessages = useChatStore((s) => s.setMessages);
  const selectChannel = useChatStore((s) => s.selectChannel);
  const incrementUnread = useChatStore((s) => s.incrementUnread);
  const setArchivedDms = useChatStore((s) => s.setArchivedDms);
  const resetChatForSwitch = useChatStore((s) => s.resetForWorkspaceSwitch);
  const setAgents = useAgentStore((s) => s.setAgents);
  const resetAgentsForSwitch = useAgentStore((s) => s.resetForWorkspaceSwitch);
  const setBoards = useBoardStore((s) => s.setBoards);
  const setSelectedBoard = useBoardStore((s) => s.setSelectedBoard);
  const resetBoardsForSwitch = useBoardStore((s) => s.resetForWorkspaceSwitch);
  const setCards = useCardStore((s) => s.setCards);
  const setArchivedCards = useCardStore((s) => s.setArchivedCards);
  const setCardMessages = useCardStore((s) => s.setCardMessages);
  const upsertCard = useCardStore((s) => s.upsertCard);
  const upsertArchivedCard = useCardStore((s) => s.upsertArchivedCard);
  const mergeCards = useCardStore((s) => s.mergeCards);
  const addCardMessages = useCardStore((s) => s.addCardMessages);
  const setShowArchived = useCardStore((s) => s.setShowArchived);
  const resetCardsForSwitch = useCardStore((s) => s.resetForWorkspaceSwitch);
  const port = useConnectionStore((s) => s.port);
  const mode = useConnectionStore((s) => s.mode);
  const localReady = useConnectionStore((s) => s.localReady);
  const setHeadCommit = useConnectionStore((s) => s.setHeadCommit);
  const setConnectionStatus = useConnectionStore((s) => s.setStatus);
  const setConnectionError = useConnectionStore((s) => s.setError);
  const isMobile = useIsMobile();
  const location = useLocation();

  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspacesLoading = useWorkspaceStore((s) => s.loading);
  const fetchWorkspaces = useWorkspaceStore((s) => s.fetchAll);
  const refreshAfterActiveUnavailable = useWorkspaceStore(
    (s) => s.refreshAfterActiveUnavailable,
  );
  const activeWorkspaceIdentity =
    activeSlug == null
      ? null
      : (() => {
          const workspace = workspaces.find((w) => w.slug === activeSlug);
          return workspace ? workspaceIdentity(mode, workspace) : null;
        })();

  // Mutable refs for poll loop — avoids stale closures
  const sinceRef = useRef<string | undefined>(undefined);
  const workspaceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);
  const activeSlugRef = useRef<string | null>(null);
  const locationPathRef = useRef(location.pathname);

  // Transport failures: fetch throws because the runtime port is gone.
  const consecutiveTransportFailuresRef = useRef(0);
  // Workspace/API failures: runtime still answers, but the active workspace
  // routes are returning errors (for example 404 / unknown workspace).
  const consecutiveWorkspaceFailuresRef = useRef(0);

  // Agent activity SSE is scoped to the active workspace
  useAgentActivitySSE(mode === "remote" ? activeSlug : null);

  // Keep refs in sync with stores
  useEffect(() => {
    return useChatStore.subscribe((state) => {
      currentChannelRef.current = state.currentChannel;
      channelsRef.current = state.channels;
    });
  }, []);

  useEffect(() => {
    activeSlugRef.current = activeSlug;
  }, [activeSlug]);

  useEffect(() => {
    locationPathRef.current = location.pathname;
  }, [location.pathname]);

  // Fetch workspaces once the runtime is reachable.
  useEffect(() => {
    if (mode === "remote" && !port) return;
    if (mode === "local" && !localReady) return;
    fetchWorkspaces();
  }, [mode, port, localReady, fetchWorkspaces]);

  const markConnected = useCallback(() => {
    consecutiveTransportFailuresRef.current = 0;
    consecutiveWorkspaceFailuresRef.current = 0;
    if (!useChatStore.getState().connected) {
      setConnected(true);
    }
  }, [setConnected]);

  const markWorkspaceUnavailable = useCallback(() => {
    consecutiveWorkspaceFailuresRef.current += 1;
    if (
      consecutiveWorkspaceFailuresRef.current === FAILS_UNTIL_DISCONNECTED &&
      useChatStore.getState().connected
    ) {
      setConnected(false);
    }
  }, [setConnected]);

  const markTransportUnavailable = useCallback(() => {
    consecutiveTransportFailuresRef.current += 1;
    if (
      consecutiveTransportFailuresRef.current === FAILS_UNTIL_DISCONNECTED &&
      useChatStore.getState().connected
    ) {
      setConnected(false);
    }
    if (
      consecutiveTransportFailuresRef.current === FAILS_UNTIL_STATUS_DEMOTE
    ) {
      // SetupGate re-renders ConnectForm; App unmounts and clears the
      // poll interval via the effect's cleanup.
      setConnectionStatus("disconnected");
    }
  }, [setConnected, setConnectionStatus]);

  const reloadActiveWorkspaceState = useCallback(
    async (
      slug: string,
      workspaceKey: string,
      options: {
        preserveSelection: boolean;
        isCancelled?: () => boolean;
      },
    ): Promise<boolean> => {
      const isCurrentTarget = () =>
        options.isCancelled?.() !== true &&
        slug === activeSlugRef.current &&
        workspaceKey === workspaceRef.current;
      const previousChannel = useChatStore.getState().currentChannel;

      const [
        meRes,
        channelsRes,
        usersRes,
        agentsRes,
        cardsRes,
        boardsRes,
        archivedChannelsRes,
        archivedDmsRes,
        archivedCardsRes,
      ] = await Promise.all([
        client.me(slug),
        client.channels(slug),
        client.users(slug),
        mode === "remote"
          ? client.listAgents(slug)
          : Promise.resolve({ ok: true, data: { agents: [] } }),
        client.listCards(slug),
        client.listBoards(slug),
        client.listArchivedChannels(slug),
        client.listArchivedDms(slug),
        client.listArchivedCards(slug),
      ]);

      if (!isCurrentTarget()) return false;
      if (
        [
          meRes,
          channelsRes,
          usersRes,
          agentsRes,
          cardsRes,
          boardsRes,
          archivedChannelsRes,
          archivedDmsRes,
          archivedCardsRes,
        ].some(isUnknownWorkspaceResponse)
      ) {
        await refreshAfterActiveUnavailable(slug);
        return false;
      }

      const nextChannels =
        channelsRes.ok && channelsRes.data
          ? (channelsRes.data.channels as Channel[])
          : useChatStore.getState().channels;
      const nextBoards =
        boardsRes.ok && boardsRes.data
          ? (boardsRes.data.boards as BoardSummary[])
          : useBoardStore.getState().boards;
      const state = useChatStore.getState();
      const currentHandler =
        meRes.ok && meRes.data
          ? (meRes.data.handler as string)
          : state.currentUser;
      const archivedChannels =
        archivedChannelsRes.ok && archivedChannelsRes.data
          ? (archivedChannelsRes.data.channels as Channel[])
          : state.archivedChannels;
      const archivedDms =
        archivedDmsRes.ok && archivedDmsRes.data
          ? archivedDmsRes.data.dms.map((dm) => {
              const members = dm.dm_pair_stem.includes("--")
                ? dm.dm_pair_stem.split("--")
                : [currentHandler, dm.peer].filter(Boolean).sort();
              return {
                name: dm.dm_pair_stem,
                kind: "dm" as const,
                unreadCount: 0,
                hasMention: false,
                members,
              };
            })
          : state.archivedDms;
      const selectableChannels = [
        ...nextChannels,
        ...archivedChannels,
        ...archivedDms,
      ];
      const boardState = useBoardStore.getState();
      const storedUiState = readUiState(workspaceKey);
      // Three-tier board handler resolution:
      // In-memory wins (keeps user's view stable across poll cycles); stored
      // selection is the cross-refresh source of truth when in-memory is gone;
      // first board is the last-resort fallback.
      const storedBoardHandler =
        storedUiState.boardHandler &&
        nextBoards.some((board) => board.handler === storedUiState.boardHandler)
          ? storedUiState.boardHandler
          : null;
      const selectedBoardHandler =
        boardState.selectedHandler &&
        nextBoards.some((board) => board.handler === boardState.selectedHandler)
          ? boardState.selectedHandler
          : storedBoardHandler ?? nextBoards[0]?.handler ?? null;
      const cardRoute = parseCardRoute(locationPathRef.current);

      // Three-tier channel resolution:
      // In-session preserve still wins so SSE/poll-reset doesn't clobber the
      // user's current view; stored selection is the cross-refresh source of
      // truth on first load; general → first channel is the last-resort fallback.
      let nextChannel: string | null = null;
      if (
        options.preserveSelection &&
        previousChannel &&
        selectableChannels.some((c) => c.name === previousChannel)
      ) {
        nextChannel = previousChannel;
      }
      if (nextChannel === null) {
        const storedChannel = storedUiState.channel;
        if (storedChannel && selectableChannels.some((c) => c.name === storedChannel)) {
          nextChannel = storedChannel;
        }
      }
      nextChannel ??=
        nextChannels.find((c) => c.name === "general")?.name ??
        nextChannels[0]?.name ??
        null;

      let messagesForChannel: Message[] | null = null;
      const [readRes, selectedBoardRes, cardDetailRes] = await Promise.all([
        nextChannel
          ? client.read(slug, toApiChannel(nextChannel), 50)
          : Promise.resolve(null),
        selectedBoardHandler
          ? client.showBoard(slug, selectedBoardHandler)
          : Promise.resolve(null),
        cardRoute
          ? client.readCard(slug, cardRoute.channel, cardRoute.cardId, {
              limit: 100,
            })
          : Promise.resolve(null),
      ]);

      if (!isCurrentTarget()) return false;
      if (readRes?.ok && readRes.data) {
        messagesForChannel = readRes.data.entries as Message[];
      }

      if (meRes.ok && meRes.data) setCurrentUser(meRes.data.handler as string);
      if (channelsRes.ok && channelsRes.data) setChannels(nextChannels);
      if (archivedChannelsRes.ok && archivedChannelsRes.data)
        setArchivedChannels(archivedChannels);
      if (archivedDmsRes.ok && archivedDmsRes.data) setArchivedDms(archivedDms);
      if (usersRes.ok && usersRes.data) setUsers(usersRes.data.users as string[]);
      if (agentsRes.ok && agentsRes.data) setAgents(agentsRes.data.agents as Agent[]);
      if (cardsRes.ok && cardsRes.data) {
        const cards = cardsRes.data.cards as Card[];
        if (options.preserveSelection) {
          mergeCards(cards);
        } else {
          setCards(cards);
        }
      }
      if (archivedCardsRes.ok && archivedCardsRes.data)
        setArchivedCards(archivedCardsRes.data.cards as Card[]);
      // Cards view preference is not in-session: always restore from storage,
      // even when preserveSelection is true.
      setShowArchived(storedUiState.cardsShowArchived);
      if (boardsRes.ok && boardsRes.data) setBoards(nextBoards);
      if (selectedBoardRes?.ok && selectedBoardRes.data) {
        setSelectedBoard(selectedBoardRes.data);
      }
      if (cardRoute && cardDetailRes?.ok && cardDetailRes.data) {
        if (cardDetailRes.data.archived) {
          upsertArchivedCard(cardDetailRes.data.meta as Card);
        } else {
          upsertCard(cardDetailRes.data.meta as Card);
        }
        setCardMessages(
          cardPathKey(cardRoute.channel, cardRoute.cardId),
          cardDetailRes.data.entries as Message[],
        );
      }

      if (nextChannel && nextChannel !== previousChannel) {
        selectChannel(nextChannel);
      }
      if (messagesForChannel) {
        setMessages(messagesForChannel);
      } else if (!nextChannel) {
        setMessages([]);
      }

      return (
        meRes.ok &&
        channelsRes.ok &&
        usersRes.ok &&
        agentsRes.ok &&
        cardsRes.ok &&
        boardsRes.ok &&
        archivedChannelsRes.ok &&
        archivedDmsRes.ok &&
        archivedCardsRes.ok &&
        (readRes === null || readRes.ok) &&
        (selectedBoardRes === null || selectedBoardRes.ok) &&
        (cardDetailRes === null || cardDetailRes.ok)
      );
    },
    [
      mode,
      refreshAfterActiveUnavailable,
      setCurrentUser,
      setChannels,
      setArchivedChannels,
      setArchivedDms,
      setUsers,
      setAgents,
      setCards,
      mergeCards,
      setArchivedCards,
      setShowArchived,
      setCardMessages,
      upsertCard,
      upsertArchivedCard,
      setBoards,
      setSelectedBoard,
      selectChannel,
      setMessages,
    ],
  );

  const runPoll = useCallback(async (signal?: AbortSignal) => {
    const slug = activeSlugRef.current;
    const requestWorkspaceKey = workspaceRef.current;
    if (!slug || !requestWorkspaceKey) return;
    const isCurrentPollTarget = () =>
      slug === activeSlugRef.current &&
      requestWorkspaceKey === workspaceRef.current;
    try {
      const pollRes = await client.poll(slug, sinceRef.current, signal);
      if (!isCurrentPollTarget()) return;

      if (!pollRes.ok || !pollRes.data) {
        if (isUnknownWorkspaceResponse(pollRes)) {
          await refreshAfterActiveUnavailable(slug);
          return;
        }
        // Stale cursor recovery: discard and re-init
        if (pollRes.error && workspaceRef.current) {
          clearCursor(workspaceRef.current);
          sinceRef.current = undefined;
        }
        markWorkspaceUnavailable();
        return;
      }

      const nextCommitId = pollRes.data.commit_id;

      if (mode === "local" && pollRes.data.needs_token === true) {
        sinceRef.current = nextCommitId;
        saveCursor(requestWorkspaceKey, sinceRef.current);
        setHeadCommit(sinceRef.current);
        setConnected(false);
        setConnectionStatus("disconnected");
        setConnectionError("Reconnect token to sync this browser workspace.");
        await fetchWorkspaces();
        return;
      }

      if (pollRes.data.reset === true) {
        clearCursor(requestWorkspaceKey);
        sinceRef.current = undefined;
        const reloaded = await reloadActiveWorkspaceState(
          slug,
          requestWorkspaceKey,
          { preserveSelection: true },
        );
        if (reloaded) {
          sinceRef.current = nextCommitId;
          saveCursor(requestWorkspaceKey, sinceRef.current);
          setHeadCommit(sinceRef.current);
          markConnected();
        }
        return;
      }

      sinceRef.current = nextCommitId;
      saveCursor(requestWorkspaceKey, sinceRef.current);
      setHeadCommit(sinceRef.current);
      markConnected();

      const changes = (pollRes.data.changes ?? []) as PollChange[];

      let needChannelRefresh = false;
      let needArchivedRefresh = false;
      let needCardRefresh = false;
      let needBoardRefresh = false;

      for (const change of changes) {
        if (change.kind === "board") {
          needBoardRefresh = true;
          continue;
        }

        // Card events: channel string is "card:<channel>/<card_id>"
        if (change.kind === "card_meta" || change.kind === "card_thread") {
          if (change.kind === "card_meta") {
            // Only meta changes (status/labels/assignee/creation) require a
            // list refresh; thread-only changes are applied in-place below.
            needCardRefresh = true;
          } else if (change.entries?.length) {
            const parsed = parseCardScope(change.channel);
            if (parsed) {
              const pathKey = `${parsed.channel}/${parsed.cardId}`;
              addCardMessages(pathKey, change.entries as Message[]);
            }
          }
          continue;
        }

        const displayName = apiToDisplay(change.channel);
        const knownChannel = channelsRef.current.some(
          (c) => c.name === displayName
        );

        if (!knownChannel || change.kind === "channel_meta") {
          needChannelRefresh = true;
          // An unknown channel that produced activity + a meta event is
          // almost certainly one that was created and archived out-of-band
          // (e.g. by an agent). Refetch archived so the record shows up
          // there instead of just vanishing from the UI.
          if (change.kind === "channel_meta" || !knownChannel) {
            needArchivedRefresh = true;
          }
          if (!knownChannel) continue;
        }

        if (displayName === currentChannelRef.current) {
          if (change.entries?.length) {
            addMessages(change.entries as Message[]);
          }
        } else {
          // Filter out self-authored entries before counting unread: after
          // sending a message and switching channels, poll echoes our own
          // send back, which would otherwise bump an unread marker on the
          // channel we just left. Self-mentions don't count as a ping either.
          const me = useChatStore.getState().currentUser;
          const othersEntries = ((change.entries ?? []) as Message[]).filter(
            (e) => e.author !== me
          );
          if (othersEntries.length === 0) continue;
          const mentionTag = `<@${me}>`;
          const mentioned = othersEntries.some((e) =>
            e.body?.includes(mentionTag)
          );
          incrementUnread(displayName, mentioned);
        }
      }

      if (needChannelRefresh) {
        const chRes = await client.channels(slug);
        if (!isCurrentPollTarget()) return;
        if (chRes.ok && chRes.data) {
          setChannels(chRes.data.channels as Channel[]);
        }
      }

      if (needArchivedRefresh) {
        const arRes = await client.listArchivedChannels(slug);
        if (!isCurrentPollTarget()) return;
        if (arRes.ok && arRes.data) {
          setArchivedChannels(arRes.data.channels as Channel[]);
        }
      }

      if (needCardRefresh) {
        const cardRes = await client.listCards(slug);
        if (!isCurrentPollTarget()) return;
        if (cardRes.ok && cardRes.data) {
          // Merge, not replace — preserves in-flight optimistic patches so
          // the 3s poll cadence can't flicker the UI back before PATCH resolves.
          mergeCards(cardRes.data.cards as Card[]);
        }
      }

      if (needBoardRefresh) {
        const boardRes = await client.listBoards(slug);
        if (!isCurrentPollTarget()) return;
        if (boardRes.ok && boardRes.data) {
          setBoards(boardRes.data.boards as BoardSummary[]);
          const selected = useBoardStore.getState().selectedHandler;
          if (selected) {
            const detailRes = await client.showBoard(slug, selected);
            if (!isCurrentPollTarget()) return;
            if (detailRes.ok && detailRes.data) {
              setSelectedBoard(detailRes.data);
            }
          }
        }
      }

      if (mode === "remote") {
        const agentsRes = await client.listAgents(slug);
        if (!isCurrentPollTarget()) return;
        if (agentsRes.ok && agentsRes.data) {
          setAgents(agentsRes.data.agents as Agent[]);
        }
      }

      // Periodically refresh the roster so DM/Create-Channel pickers see
      // agents that were provisioned mid-session (on this or another clone).
      // Initial `client.users` ran once during init; without a refresh the
      // list stays frozen and new members look invisible to the UI.
      // Daemon returns the list sorted → equal-length + index-wise equal
      // is a sufficient change check.
      const usersRes = await client.users(slug);
      if (!isCurrentPollTarget()) return;
      if (usersRes.ok && usersRes.data) {
        const next = usersRes.data.users as string[];
        const current = useChatStore.getState().users;
        const changed =
          next.length !== current.length ||
          next.some((u, i) => u !== current[i]);
        if (changed) setUsers(next);
      }
    } catch (err) {
      // AbortError is our own timeout — not a real transport failure.
      if (err instanceof DOMException && err.name === "AbortError") return;

      // Connectivity-level failure (fetch threw). Race guard: a poll that
      // started for slug A shouldn't flip slug B's state if the user
      // switched workspaces mid-request.
      if (slug !== activeSlugRef.current) return;

      markTransportUnavailable();
    }
  }, [
    addMessages,
    incrementUnread,
    setChannels,
    setArchivedChannels,
    setAgents,
    setUsers,
    setBoards,
    setSelectedBoard,
    mergeCards,
    addCardMessages,
    setHeadCommit,
    setConnected,
    setConnectionStatus,
    setConnectionError,
    fetchWorkspaces,
    refreshAfterActiveUnavailable,
    markConnected,
    markWorkspaceUnavailable,
    markTransportUnavailable,
    reloadActiveWorkspaceState,
    mode,
  ]);

  // Init + poll loop — runs whenever port + activeSlug are both set, and
  // re-runs whenever activeSlug changes so state is refreshed on switch.
  useEffect(() => {
    if (!activeSlug) return;
    if (mode === "remote" && !port) return;
    if (mode === "local" && !localReady) return;
    if (!activeWorkspaceIdentity) return;
    const workspaceKey = activeWorkspaceIdentity;

    // Reset per-workspace store slices on switch so stale data from the
    // previous workspace doesn't leak into the new one. Each store owns
    // the knowledge of which of its fields are workspace-scoped — in
    // particular chat resets `currentChannel` + `messages` so poll-driven
    // `addMessages` can't append ws-B entries onto ws-A's list.
    resetChatForSwitch();
    resetAgentsForSwitch();
    resetCardsForSwitch();
    resetBoardsForSwitch();
    sinceRef.current = undefined;
    workspaceRef.current = undefined;
    consecutiveTransportFailuresRef.current = 0;
    consecutiveWorkspaceFailuresRef.current = 0;
    setHeadCommit(null);

    // Guard against React 19 Strict Mode's simulated unmount: if cleanup
    // ran before init() resolved, skip the setInterval so we don't leak an
    // orphan poll loop that keeps firing alongside the real mount's loop.
    let cancelled = false;
    let pollHandle: ReturnType<typeof setTimeout> | undefined;

    async function init(slug: string): Promise<boolean> {
      let activationNeedsToken = false;
      if (mode === "local") {
        const activation = await client.activateBrowserWorkspace(slug, {
          onSyncReset: () => {
            void reloadActiveWorkspaceState(slug, workspaceKey, {
              preserveSelection: true,
            }).catch(() => {
              markTransportUnavailable();
            });
          },
        });
        if (cancelled) return false;
        if (activation.error_code === "activation_superseded") return false;
        if (!activation.ok) {
          setConnectionStatus("disconnected");
          setConnectionError(activation.error ?? "Failed to activate browser workspace");
          setConnected(false);
          return false;
        }
        activationNeedsToken = activation.data?.needs_token === true;
      }

      // Restore cursor from localStorage keyed by runtime or browser workspace identity.
      workspaceRef.current = workspaceKey;
      sinceRef.current = loadCursor(workspaceKey);
      const bootstrapOk = await reloadActiveWorkspaceState(
        slug,
        workspaceKey,
        {
          preserveSelection: false,
          isCancelled: () => cancelled,
        },
      );

      if (cancelled) return false;

      if (activationNeedsToken) {
        setConnected(false);
        setConnectionStatus("disconnected");
        setConnectionError("Reconnect token to sync this browser workspace.");
        await fetchWorkspaces();
        return false;
      }

      if (bootstrapOk) {
        markConnected();
      }
      return true;
    }

    init(activeSlug).then((readyToPoll) => {
      if (cancelled || !readyToPoll) return;

      // Recursive setTimeout instead of setInterval: ensures a single in-flight
      // poll at a time. With setInterval, a fetch that stalls past the 3s
      // cadence would pile concurrent callbacks on top of each other.
      const pollInterval =
        mode === "local" ? LOCAL_POLL_INTERVAL_MS : POLL_INTERVAL_MS;
      const schedulePoll = () => {
        if (cancelled) return;
        pollHandle = setTimeout(async () => {
          if (cancelled) return;
          const controller = new AbortController();
          const timeoutHandle = setTimeout(() => controller.abort(), 8000);
          try {
            await runPoll(controller.signal);
          } finally {
            clearTimeout(timeoutHandle);
            schedulePoll();
          }
        }, pollInterval);
      };
      schedulePoll();
    }).catch(() => {
      if (!cancelled) markTransportUnavailable();
    });

    return () => {
      cancelled = true;
      if (pollHandle !== undefined) clearTimeout(pollHandle);
    };
  }, [
    port,
    mode,
    localReady,
    activeSlug,
    activeWorkspaceIdentity,
    setCurrentUser,
    setChannels,
    setUsers,
    setAgents,
    setCards,
    setBoards,
    setHeadCommit,
    resetChatForSwitch,
    resetAgentsForSwitch,
    resetCardsForSwitch,
    resetBoardsForSwitch,
    setConnected,
    setConnectionStatus,
    setConnectionError,
    fetchWorkspaces,
    markConnected,
    markTransportUnavailable,
    reloadActiveWorkspaceState,
    runPoll,
  ]);

  // /docs is a standalone reference — let it render regardless of workspace
  // state so setup-screen hints ("What scopes does the PAT need?") can deep-link
  // into it without getting bounced back by the gate.
  const isDocsRoute = location.pathname.startsWith("/docs");

  // Render-time gate: until we have a workspace selected, bypass the chat UI.
  let gated: React.ReactNode;
  if (isDocsRoute) {
    gated = <DocsPage />;
  } else if (mode === "local" && workspaces.length === 0) {
    gated = <WorkspaceLoading />;
  } else if (workspacesLoading && workspaces.length === 0) {
    gated = <WorkspaceLoading />;
  } else if (workspaces.length === 0) {
    gated = <FirstRunScreen />;
  } else {
    const active = activeSlug
      ? workspaces.find((w) => w.slug === activeSlug)
      : null;
    if (active && !active.initialized) {
      gated = <WorkspaceIncomplete slug={active.slug} />;
    } else {
      gated = (
        <Routes>
          <Route element={<AppShell />}>
            <Route
              index
              element={
                <Navigate
                  to={mode === "local" || isMobile ? "/chat" : "/management"}
                  replace
                />
              }
            />
            {mode === "remote" && (
              <>
                <Route
                  path="/management"
                  element={
                    isMobile ? <Navigate to="/chat" replace /> : <ManagementPage />
                  }
                />
                <Route
                  path="/management/:agentId"
                  element={
                    isMobile ? <Navigate to="/chat" replace /> : <AgentDetail />
                  }
                />
              </>
            )}
            <Route path="/cards" element={<CardKanban />} />
            <Route path="/cards/:channel/:card_id" element={<CardDetail />} />
            <Route path="/boards" element={<BoardsView />} />
            <Route path="/chat" element={<ChatPage />} />
            {mode === "remote" && (
              <Route path="/crons" element={<CronCalendar />} />
            )}
            <Route path="/docs" element={<DocsPage />} />
            {mode === "local" && (
              <Route path="*" element={<Navigate to="/chat" replace />} />
            )}
          </Route>
        </Routes>
      );
    }
  }

  return (
    <SetupGate>
      <Toaster
        position={isMobile ? "bottom-center" : "top-right"}
        mobileOffset={{
          bottom: "calc(env(safe-area-inset-bottom) + 72px)",
          left: 16,
          right: 16,
        }}
        richColors
      />
      {gated}
    </SetupGate>
  );
}
