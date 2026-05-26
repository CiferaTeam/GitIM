import { useEffect, useRef } from "react";
import { useLocation, useNavigate } from "react-router";
import { toast } from "sonner";
import * as client from "../lib/client";
import {
  chatScopeKeyForName,
  chatScopeName,
  incrementChatScopeUnread,
  mergeChatUnreadIntoChannels,
  readActiveChatScope,
  readChatScopeState,
  writeActiveChatScope,
} from "../lib/chat-ui-state";
import { computeAnchoredReadSince } from "../components/chat/pagination";
import { clearCursor, loadCursor, saveCursor } from "../lib/cursor";
import { resolveRemoteSyncFromChanges } from "../lib/remote-sync-toast";
import type {
  Agent,
  ApiResponse,
  BoardSummary,
  Card,
  Channel,
  FleetAgentSnapshot,
  FleetNodeStatus,
  Message,
  PollChange,
} from "../lib/types";
import { readUiState } from "../lib/ui-state";
import { workspaceIdentity } from "../lib/workspace-key";
import { emitWorkspaceSwitch } from "../lib/workspace-lifecycle";
import { useAgentStore } from "./use-agent-store";
import { useBoardStore } from "./use-board-store";
import { cardPathKey, parseCardScope, useCardStore } from "./use-card-store";
import { useChatStore } from "./use-chat-store";
import { useConnectionDiagnosticsStore } from "./use-connection-diagnostics-store";
import { useConnectionStore } from "./use-connection-store";
import { useFleetStore } from "./use-fleet-store";
import { useWorkspaceStore } from "./use-workspace-store";

const POLL_INTERVAL_MS = 3000;
const LOCAL_POLL_INTERVAL_MS = 7000;
const REQUEST_TIMEOUT_MS = 8000;

// Consecutive connectivity failures (fetch-level) before we flip the
// header dot red. At 3s cadence, 3 fails ≈ 9s of unreachability.
const FAILS_UNTIL_DISCONNECTED = 3;

// After this many consecutive fails, demote connection status back to
// "disconnected" so SetupGate re-renders ConnectForm and the user gets
// an actionable reconnect path. At 3s cadence, 10 fails ≈ 30s — enough
// room for a quick runtime restart before we kick the user out.
const FAILS_UNTIL_STATUS_DEMOTE = 10;

type AgentSnapshotResponses = {
  agentsRes: ApiResponse<{ agents: Agent[] }>;
  fleetAgentsRes: ApiResponse<{ agents: FleetAgentSnapshot[] }>;
  fleetStatusRes: ApiResponse<{ nodes: FleetNodeStatus[] }>;
};

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

function parseCardRoute(
  pathname: string,
): { channel: string; cardId: string } | null {
  const match = /^\/cards\/([^/]+)\/([^/]+)\/?$/.exec(pathname);
  if (!match) return null;
  return {
    channel: decodePathSegment(match[1]),
    cardId: decodePathSegment(match[2]),
  };
}

function isUnknownWorkspaceResponse(res: {
  ok: boolean;
  error?: string | null;
}): boolean {
  return !res.ok && res.error === "unknown workspace";
}

function isChatRoute(pathname: string): boolean {
  return pathname === "/chat" || pathname.startsWith("/chat/");
}

/**
 * Owns the workspace lifecycle: bootstrap (me/channels/users/agents/cards/boards),
 * recursive poll loop, SSE-driven sync resets, and per-workspace store resets.
 *
 * Pure side-effect hook — no returns. Subscribes to a minimal slice of stores
 * (the 5 trigger inputs: activeSlug, mode, port, localReady, activeWorkspaceIdentity)
 * and reads everything else via `getState()` so callbacks never need to land in
 * dependency arrays. This stops the "one unstable hook ref poisons the whole
 * init effect" pattern that broke chat scroll restoration across route changes.
 *
 * Architectural rule for this file: store *actions* are accessed via
 * `useXxxStore.getState().action(...)` rather than selector subscriptions.
 * Selectors here would re-render the host component and rebuild the callbacks
 * the effect closes over, which defeats the whole point.
 */
export function usePollLoop(): void {
  // react-router v7's `useNavigate` returns a fresh function reference whenever
  // the location changes. Hold it via a ref so the toast action handler always
  // sees the latest navigator without putting it on the dep graph.
  const navigate = useNavigate();
  const navigateRef = useRef(navigate);
  navigateRef.current = navigate;

  // Same shape for location: pathname is read inside runPoll's per-change
  // visibility check, so we keep a ref instead of closing over a value that
  // would stale-rot through async awaits.
  const location = useLocation();
  const locationPathRef = useRef(location.pathname);
  locationPathRef.current = location.pathname;

  // The five real triggers for re-init. Everything else is read via getState().
  const mode = useConnectionStore((s) => s.mode);
  const port = useConnectionStore((s) => s.port);
  const localReady = useConnectionStore((s) => s.localReady);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeWorkspaceIdentity =
    activeSlug == null
      ? null
      : (() => {
          const workspace = workspaces.find((w) => w.slug === activeSlug);
          return workspace ? workspaceIdentity(mode, workspace) : null;
        })();

  // Mutable refs for the poll loop — avoid stale closures across awaits.
  const sinceRef = useRef<string | undefined>(undefined);
  const workspaceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);
  const activeSlugRef = useRef<string | null>(null);
  const agentSnapshotRequestRef = useRef<{
    workspaceKey: string;
    promise: Promise<AgentSnapshotResponses | null>;
  } | null>(null);

  // Transport failures: fetch throws because the runtime port is gone.
  const consecutiveTransportFailuresRef = useRef(0);
  // Workspace/API failures: runtime still answers, but the active workspace
  // routes are returning errors (for example 404 / unknown workspace).
  const consecutiveWorkspaceFailuresRef = useRef(0);

  // Keep refs in sync with stores so async callbacks see latest values.
  useEffect(() => {
    return useChatStore.subscribe((state) => {
      currentChannelRef.current = state.currentChannel;
      channelsRef.current = state.channels;
    });
  }, []);

  useEffect(() => {
    activeSlugRef.current = activeSlug;
  }, [activeSlug]);

  // Fetch workspaces once the runtime is reachable.
  useEffect(() => {
    if (mode === "remote" && !port) return;
    if (mode === "local" && !localReady) return;
    useWorkspaceStore.getState().fetchAll();
  }, [mode, port, localReady]);

  // --- Connection-state helpers ---------------------------------------------
  //
  // These mutate refs + the connection-status store. They're declared as
  // closures-over-refs so we can call them from anywhere in this hook without
  // adding them to a dep array.

  function markConnected(commitId?: string | null): void {
    consecutiveTransportFailuresRef.current = 0;
    consecutiveWorkspaceFailuresRef.current = 0;
    useConnectionDiagnosticsStore.getState().recordPollSuccess(commitId);
    if (!useChatStore.getState().connected) {
      useChatStore.getState().setConnected(true);
    }
  }

  function markWorkspaceUnavailable(error?: unknown): void {
    consecutiveWorkspaceFailuresRef.current += 1;
    useConnectionDiagnosticsStore
      .getState()
      .recordPollFailure(
        "workspace",
        error ?? "Workspace routes are unavailable",
      );
    if (
      consecutiveWorkspaceFailuresRef.current === FAILS_UNTIL_DISCONNECTED &&
      useChatStore.getState().connected
    ) {
      useChatStore.getState().setConnected(false);
    }
  }

  function markTransportUnavailable(error?: unknown): void {
    consecutiveTransportFailuresRef.current += 1;
    useConnectionDiagnosticsStore
      .getState()
      .recordPollFailure(
        "transport",
        error ?? "Runtime transport is unavailable",
      );
    if (
      consecutiveTransportFailuresRef.current === FAILS_UNTIL_DISCONNECTED &&
      useChatStore.getState().connected
    ) {
      useChatStore.getState().setConnected(false);
    }
    if (consecutiveTransportFailuresRef.current === FAILS_UNTIL_STATUS_DEMOTE) {
      // SetupGate re-renders ConnectForm; the main effect's cleanup clears the
      // poll interval before re-init.
      useConnectionStore.getState().setStatus("disconnected");
    }
  }

  // --- Agent snapshot dedup -------------------------------------------------

  function fetchAgentSnapshots(
    slug: string,
    workspaceKey: string,
  ): Promise<AgentSnapshotResponses | null> {
    if (mode !== "remote") return Promise.resolve(null);

    const inFlight = agentSnapshotRequestRef.current;
    if (inFlight?.workspaceKey === workspaceKey) return inFlight.promise;

    const promise = Promise.all([
      client.listAgents(slug) as Promise<ApiResponse<{ agents: Agent[] }>>,
      client.listFleetAgents(),
      client.listFleetStatus(),
    ])
      .then(([agentsRes, fleetAgentsRes, fleetStatusRes]) => ({
        agentsRes,
        fleetAgentsRes,
        fleetStatusRes,
      }))
      .finally(() => {
        if (agentSnapshotRequestRef.current?.promise === promise) {
          agentSnapshotRequestRef.current = null;
        }
      });

    agentSnapshotRequestRef.current = { workspaceKey, promise };
    return promise;
  }

  function applyAgentSnapshots(snapshot: AgentSnapshotResponses | null): void {
    if (!snapshot) return;
    const { agentsRes, fleetAgentsRes, fleetStatusRes } = snapshot;
    const agentStore = useAgentStore.getState();
    const fleetStore = useFleetStore.getState();
    if (agentsRes.ok && agentsRes.data) agentStore.setAgents(agentsRes.data.agents);
    if (fleetAgentsRes.ok && fleetAgentsRes.data) {
      fleetStore.setAgents(fleetAgentsRes.data.agents);
    }
    if (fleetStatusRes.ok && fleetStatusRes.data) {
      fleetStore.setStatuses(fleetStatusRes.data.nodes);
    }
  }

  // --- Workspace reload (bootstrap + sync-reset path) ----------------------

  async function reloadActiveWorkspaceState(
    slug: string,
    workspaceKey: string,
    options: {
      preserveSelection: boolean;
      isCancelled?: () => boolean;
    },
  ): Promise<boolean> {
    const isCurrentTarget = (): boolean =>
      options.isCancelled?.() !== true &&
      slug === activeSlugRef.current &&
      workspaceKey === workspaceRef.current;
    const previousChannel = useChatStore.getState().currentChannel;

    // Archived channels / DMs are *not* fetched here — they're lazy-loaded
    // by the sidebar on first expand (and paginated + prefix-filtered server
    // side). Including them in this bootstrap pre-loaded the entire archive
    // on every workspace activation, which doesn't scale.
    const [
      meRes,
      channelsRes,
      usersRes,
      agentSnapshot,
      cardsRes,
      boardsRes,
      archivedCardsRes,
    ] = await Promise.all([
      client.me(slug),
      client.channels(slug),
      client.users(slug),
      fetchAgentSnapshots(slug, workspaceKey),
      client.listCards(slug),
      client.listBoards(slug),
      client.listArchivedCards(slug),
    ]);

    if (!isCurrentTarget()) return false;
    if (
      [
        meRes,
        channelsRes,
        usersRes,
        ...(agentSnapshot ? [agentSnapshot.agentsRes] : []),
        cardsRes,
        boardsRes,
        archivedCardsRes,
      ].some(isUnknownWorkspaceResponse)
    ) {
      await useWorkspaceStore.getState().refreshAfterActiveUnavailable(slug);
      return false;
    }

    const chatStore = useChatStore.getState();
    const boardStoreState = useBoardStore.getState();
    const cardStore = useCardStore.getState();

    const nextChannels =
      channelsRes.ok && channelsRes.data
        ? mergeChatUnreadIntoChannels(
            workspaceKey,
            channelsRes.data.channels as Channel[],
          )
        : chatStore.channels;
    const nextBoards =
      boardsRes.ok && boardsRes.data
        ? (boardsRes.data.boards as BoardSummary[])
        : boardStoreState.boards;
    const archivedChannels =
      chatStore.archivedChannelsView?.items ?? chatStore.archivedChannels;
    const selectableChannels = [...nextChannels, ...archivedChannels];
    // DM stems look like `<min>--<max>`. We can't probe the archive view for
    // them at bootstrap (lazy-loaded), so treat any `--`-shaped name as
    // potentially-selectable — daemon's read handler falls back to archive/
    // automatically, so if the DM is gone the user will see an empty
    // timeline rather than getting silently bounced to general.
    const isDmStem = (name: string): boolean =>
      name.includes("--") && !nextChannels.some((c) => c.name === name);
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
      boardStoreState.selectedHandler &&
      nextBoards.some(
        (board) => board.handler === boardStoreState.selectedHandler,
      )
        ? boardStoreState.selectedHandler
        : (storedBoardHandler ?? nextBoards[0]?.handler ?? null);
    const cardRoute = parseCardRoute(locationPathRef.current);

    // Three-tier channel resolution:
    // In-session preserve still wins so SSE/poll-reset doesn't clobber the
    // user's current view; stored selection is the cross-refresh source of
    // truth on first load; general → first channel is the last-resort
    // fallback. Archived DMs are accepted via `isDmStem` since they're no
    // longer in the bootstrap payload — the read endpoint falls back to
    // archive content.
    const isSelectable = (name: string): boolean =>
      selectableChannels.some((c) => c.name === name) || isDmStem(name);
    let nextChannel: string | null = null;
    if (
      options.preserveSelection &&
      previousChannel &&
      isSelectable(previousChannel)
    ) {
      nextChannel = previousChannel;
    }
    if (nextChannel === null) {
      const storedChannel = chatScopeName(readActiveChatScope(workspaceKey));
      if (storedChannel && isSelectable(storedChannel)) {
        nextChannel = storedChannel;
      }
    }
    nextChannel ??=
      nextChannels.find((c) => c.name === "general")?.name ??
      nextChannels[0]?.name ??
      null;

    let messagesForChannel: Message[] | null = null;
    let pendingLineForChannel: number | null = null;
    let readSinceForChannel: number | undefined;
    const nextChannelScopeKey = nextChannel
      ? chatScopeKeyForName(
          nextChannel,
          selectableChannels.find((c) => c.name === nextChannel)?.kind,
        )
      : null;
    if (nextChannel && nextChannelScopeKey) {
      const shouldRestoreStoredPosition =
        !options.preserveSelection || previousChannel !== nextChannel;
      if (shouldRestoreStoredPosition) {
        const scopeState = readChatScopeState(
          workspaceKey,
          nextChannelScopeKey,
        );
        pendingLineForChannel =
          scopeState.unreadCount > 0 ? scopeState.firstUnreadLine : null;
        readSinceForChannel = pendingLineForChannel
          ? computeAnchoredReadSince(pendingLineForChannel)
          : undefined;
      }
    }
    const [readRes, selectedBoardRes, cardDetailRes] = await Promise.all([
      nextChannel
        ? client.read(slug, toApiChannel(nextChannel), 50, readSinceForChannel)
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

    if (meRes.ok && meRes.data) {
      chatStore.setCurrentUser(meRes.data.handler as string);
    }
    if (channelsRes.ok && channelsRes.data) chatStore.setChannels(nextChannels);
    if (usersRes.ok && usersRes.data) {
      chatStore.setUsers(usersRes.data.users as string[]);
    }
    applyAgentSnapshots(agentSnapshot);
    if (cardsRes.ok && cardsRes.data) {
      const cards = cardsRes.data.cards as Card[];
      if (options.preserveSelection) {
        cardStore.mergeCards(cards);
      } else {
        cardStore.setCards(cards);
      }
    }
    if (archivedCardsRes.ok && archivedCardsRes.data) {
      cardStore.setArchivedCards(archivedCardsRes.data.cards as Card[]);
    }
    // Cards view preference is not in-session: always restore from storage,
    // even when preserveSelection is true.
    cardStore.setShowArchived(storedUiState.cardsShowArchived);
    if (boardsRes.ok && boardsRes.data) boardStoreState.setBoards(nextBoards);
    if (selectedBoardRes?.ok && selectedBoardRes.data) {
      boardStoreState.setSelectedBoard(selectedBoardRes.data);
    }
    if (cardRoute && cardDetailRes?.ok && cardDetailRes.data) {
      if (cardDetailRes.data.archived) {
        cardStore.upsertArchivedCard(cardDetailRes.data.meta as Card);
      } else {
        cardStore.upsertCard(cardDetailRes.data.meta as Card);
      }
      cardStore.setCardMessages(
        cardPathKey(cardRoute.channel, cardRoute.cardId),
        cardDetailRes.data.entries as Message[],
      );
    }

    if (nextChannel && nextChannel !== previousChannel) {
      chatStore.selectChannel(nextChannel);
    }
    if (pendingLineForChannel) {
      chatStore.setPendingScrollLine(pendingLineForChannel);
    }
    if (nextChannel) {
      writeActiveChatScope(workspaceKey, nextChannelScopeKey);
    }
    if (messagesForChannel) {
      chatStore.setMessages(messagesForChannel);
    } else if (!nextChannel) {
      chatStore.setMessages([]);
    }

    return (
      meRes.ok &&
      channelsRes.ok &&
      usersRes.ok &&
      (agentSnapshot === null || agentSnapshot.agentsRes.ok) &&
      cardsRes.ok &&
      boardsRes.ok &&
      archivedCardsRes.ok &&
      (readRes === null || readRes.ok) &&
      (selectedBoardRes === null || selectedBoardRes.ok) &&
      (cardDetailRes === null || cardDetailRes.ok)
    );
  }

  // --- The poll cycle -------------------------------------------------------

  async function runPoll(signal?: AbortSignal): Promise<void> {
    const slug = activeSlugRef.current;
    const requestWorkspaceKey = workspaceRef.current;
    if (!slug || !requestWorkspaceKey) return;
    const isCurrentPollTarget = (): boolean =>
      slug === activeSlugRef.current &&
      requestWorkspaceKey === workspaceRef.current;
    try {
      const pollRes = await client.poll(slug, sinceRef.current, signal);
      if (!isCurrentPollTarget()) return;

      if (!pollRes.ok || !pollRes.data) {
        if (isUnknownWorkspaceResponse(pollRes)) {
          await useWorkspaceStore
            .getState()
            .refreshAfterActiveUnavailable(slug);
          return;
        }
        // Stale cursor recovery: discard and re-init.
        if (pollRes.error && workspaceRef.current) {
          clearCursor(workspaceRef.current);
          sinceRef.current = undefined;
        }
        markWorkspaceUnavailable(pollRes.error ?? "Poll failed");
        return;
      }

      const nextCommitId = pollRes.data.commit_id;

      if (mode === "local" && pollRes.data.needs_token === true) {
        sinceRef.current = nextCommitId;
        saveCursor(requestWorkspaceKey, sinceRef.current);
        useConnectionStore.getState().setHeadCommit(sinceRef.current);
        useConnectionDiagnosticsStore
          .getState()
          .recordPollFailure(
            "token",
            "Reconnect token to sync this browser workspace.",
          );
        useChatStore.getState().setConnected(false);
        useConnectionStore.getState().setStatus("disconnected");
        useConnectionStore
          .getState()
          .setError("Reconnect token to sync this browser workspace.");
        await useWorkspaceStore.getState().fetchAll();
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
          useConnectionStore.getState().setHeadCommit(sinceRef.current);
          markConnected(nextCommitId);
        }
        return;
      }

      sinceRef.current = nextCommitId;
      saveCursor(requestWorkspaceKey, sinceRef.current);
      useConnectionStore.getState().setHeadCommit(sinceRef.current);
      markConnected(nextCommitId);

      const changes = (pollRes.data.changes ?? []) as PollChange[];
      resolveRemoteSyncFromChanges(requestWorkspaceKey, changes);

      let needChannelRefresh = false;
      let needArchivedChannelInvalidate = false;
      let needCardRefresh = false;
      let needBoardRefresh = false;

      const chatActions = useChatStore.getState();
      const cardActions = useCardStore.getState();

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
            // No toast for card_meta — the resulting Kanban/list re-render
            // is itself the awareness signal, so a toast would be redundant
            // noise that also lacks a self-filter (PollChange has no
            // top-level author).
            needCardRefresh = true;
          } else if (change.entries?.length) {
            const parsed = parseCardScope(change.channel);
            if (parsed) {
              const pathKey = `${parsed.channel}/${parsed.cardId}`;
              cardActions.addCardMessages(pathKey, change.entries as Message[]);
            }
            // Awareness toast for new discussion messages — unlike meta
            // changes, thread updates have no natural visual surface unless
            // the card detail is open. Filter to others-only (entries carry
            // author); pairs with the self-filter idiom for unread counting
            // below.
            const me = useChatStore.getState().currentUser;
            const others = (change.entries as Message[]).filter(
              (e) => e.author !== me,
            );
            if (others.length > 0 && parsed) {
              const shortId = parsed.cardId.slice(0, 8);
              const authors = Array.from(
                new Set(others.map((e) => `@${e.author}`)),
              ).join(", ");
              const noun =
                others.length === 1
                  ? "new message"
                  : `${others.length} new messages`;
              toast.info(`Card #${shortId}: ${noun} from ${authors}`, {
                action: {
                  label: "Open card",
                  onClick: () => {
                    navigateRef.current(
                      `/cards/${encodeURIComponent(parsed.channel)}/${encodeURIComponent(parsed.cardId)}`,
                    );
                  },
                },
              });
            }
          }
          continue;
        }

        const displayName = apiToDisplay(change.channel);
        const isDmChange =
          change.kind === "dm" ||
          change.kind === "dm_archived" ||
          change.channel.startsWith("dm:") ||
          (change.kind === "new_messages" && displayName.includes("--"));

        if (change.kind === "dm_archived") {
          chatActions.markDmArchived(displayName);
          needChannelRefresh = true;
          continue;
        }

        const knownChannel = channelsRef.current.some(
          (c) => c.name === displayName,
        );

        if (!knownChannel && isDmChange) {
          // Active DM file reappeared (unarchive) or a new DM arrived while
          // this clone had no local Channel entry. Seed it immediately so
          // unread increments / sidebar visibility work before the slower
          // channels() refresh returns.
          chatActions.markDmUnarchived(displayName);
          needChannelRefresh = true;
        }

        if (!knownChannel || change.kind === "channel_meta") {
          needChannelRefresh = true;
          // Channel archive/unarchive should not force-load the whole
          // archive list. Invalidate the lazy view; the open sidebar
          // refetches page 1.
          if (
            !isDmChange &&
            (change.kind === "channel_meta" || !knownChannel)
          ) {
            needArchivedChannelInvalidate = true;
            // Channel archive/unarchive flips which channels listCards
            // scans (s.channels = active only). Without this refresh the
            // kanban keeps showing cards from a now-archived channel until
            // some other card_meta change triggers a refetch.
            needCardRefresh = true;
          }
          if (!knownChannel && !isDmChange) continue;
        }

        const selectedChannelVisible =
          displayName === currentChannelRef.current &&
          isChatRoute(locationPathRef.current);
        if (displayName === currentChannelRef.current) {
          if (change.entries?.length) {
            chatActions.addMessages(change.entries as Message[]);
          }
        }
        if (!selectedChannelVisible) {
          // Filter out self-authored entries before counting unread: after
          // sending a message and switching channels, poll echoes our own
          // send back, which would otherwise bump an unread marker on the
          // channel we just left. Self-mentions don't count as a ping
          // either.
          const me = useChatStore.getState().currentUser;
          const othersEntries = ((change.entries ?? []) as Message[]).filter(
            (e) => e.author !== me,
          );
          if (othersEntries.length === 0) continue;
          const mentionTag = `<@${me}>`;
          const mentioned = othersEntries.some((e) =>
            e.body?.includes(mentionTag),
          );
          chatActions.incrementUnread(displayName, mentioned);
          const firstUnreadLine =
            othersEntries
              .map((entry) => entry.line_number)
              .filter((line) => line > 0)
              .sort((a, b) => a - b)[0] ?? null;
          incrementChatScopeUnread(
            requestWorkspaceKey,
            chatScopeKeyForName(displayName),
            {
              count: othersEntries.length,
              hasMention: mentioned,
              firstUnreadLine,
            },
          );
        }
      }

      if (needChannelRefresh) {
        const chRes = await client.channels(slug);
        if (!isCurrentPollTarget()) return;
        if (chRes.ok && chRes.data) {
          useChatStore
            .getState()
            .setChannels(
              mergeChatUnreadIntoChannels(
                requestWorkspaceKey,
                chRes.data.channels as Channel[],
              ),
            );
        }
      }

      if (needArchivedChannelInvalidate) {
        useChatStore.getState().invalidateArchivedChannelsView();
      }

      if (needCardRefresh) {
        const cardRes = await client.listCards(slug);
        if (!isCurrentPollTarget()) return;
        if (cardRes.ok && cardRes.data) {
          // Merge, not replace — preserves in-flight optimistic patches so
          // the 3s poll cadence can't flicker the UI back before PATCH
          // resolves.
          useCardStore.getState().mergeCards(cardRes.data.cards as Card[]);
        }
      }

      if (needBoardRefresh) {
        const boardRes = await client.listBoards(slug);
        if (!isCurrentPollTarget()) return;
        if (boardRes.ok && boardRes.data) {
          useBoardStore
            .getState()
            .setBoards(boardRes.data.boards as BoardSummary[]);
          const selected = useBoardStore.getState().selectedHandler;
          if (selected) {
            const detailRes = await client.showBoard(slug, selected);
            if (!isCurrentPollTarget()) return;
            if (detailRes.ok && detailRes.data) {
              useBoardStore.getState().setSelectedBoard(detailRes.data);
            }
          }
        }
      }

      // Periodically refresh the roster so DM/Create-Channel pickers see
      // agents that were provisioned mid-session (on this or another clone).
      // Initial `client.users` ran once during init; without a refresh the
      // list stays frozen and new members look invisible to the UI. Daemon
      // returns the list sorted → equal-length + index-wise equal is a
      // sufficient change check.
      const usersRes = await client.users(slug);
      if (!isCurrentPollTarget()) return;
      if (usersRes.ok && usersRes.data) {
        const next = usersRes.data.users as string[];
        const current = useChatStore.getState().users;
        const changed =
          next.length !== current.length ||
          next.some((u, i) => u !== current[i]);
        if (changed) useChatStore.getState().setUsers(next);
      }
    } catch (err) {
      // AbortError is our own timeout — not a real transport failure.
      if (err instanceof DOMException && err.name === "AbortError") return;

      // Connectivity-level failure (fetch threw). Race guard: a poll that
      // started for slug A shouldn't flip slug B's state if the user switched
      // workspaces mid-request.
      if (slug !== activeSlugRef.current) return;

      markTransportUnavailable(err);
    }
  }

  // --- Init + poll loop -----------------------------------------------------
  //
  // Re-fires only when the five real triggers change (activeSlug, workspaceKey,
  // mode, port, localReady). Everything else (store actions, navigate,
  // location) is accessed via getState() or refs, so they never poison this
  // dep array.
  useEffect(() => {
    if (!activeSlug) return;
    if (mode === "remote" && !port) return;
    if (mode === "local" && !localReady) return;
    if (!activeWorkspaceIdentity) return;
    const workspaceKey = activeWorkspaceIdentity;

    // Reset per-workspace store slices on switch so stale data from the
    // previous workspace doesn't leak into the new one. Each store registers
    // its own listener at module load (see lib/workspace-lifecycle.ts), so a
    // new workspace-scoped store can't forget to wire its reset — the wiring
    // lives next to the store. We just fire the event here.
    emitWorkspaceSwitch();
    sinceRef.current = undefined;
    workspaceRef.current = undefined;
    consecutiveTransportFailuresRef.current = 0;
    consecutiveWorkspaceFailuresRef.current = 0;
    useConnectionStore.getState().setHeadCommit(null);

    // Guard against React 19 Strict Mode's simulated unmount: if cleanup ran
    // before init() resolved, skip the setInterval so we don't leak an orphan
    // poll loop that keeps firing alongside the real mount's loop.
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
          useConnectionStore.getState().setStatus("disconnected");
          useConnectionStore
            .getState()
            .setError(
              activation.error ?? "Failed to activate browser workspace",
            );
          useConnectionDiagnosticsStore
            .getState()
            .recordPollFailure(
              "activation",
              activation.error ?? "Failed to activate browser workspace",
            );
          useChatStore.getState().setConnected(false);
          return false;
        }
        activationNeedsToken = activation.data?.needs_token === true;
      }

      // Restore cursor from localStorage keyed by runtime or browser
      // workspace identity.
      workspaceRef.current = workspaceKey;
      sinceRef.current = loadCursor(workspaceKey);
      const bootstrapOk = await reloadActiveWorkspaceState(slug, workspaceKey, {
        preserveSelection: false,
        isCancelled: () => cancelled,
      });

      if (cancelled) return false;

      if (activationNeedsToken) {
        useConnectionDiagnosticsStore
          .getState()
          .recordPollFailure(
            "token",
            "Reconnect token to sync this browser workspace.",
          );
        useChatStore.getState().setConnected(false);
        useConnectionStore.getState().setStatus("disconnected");
        useConnectionStore
          .getState()
          .setError("Reconnect token to sync this browser workspace.");
        await useWorkspaceStore.getState().fetchAll();
        return false;
      }

      if (bootstrapOk) {
        markConnected(sinceRef.current ?? null);
      }
      return true;
    }

    init(activeSlug)
      .then((readyToPoll) => {
        if (cancelled || !readyToPoll) return;

        // Recursive setTimeout instead of setInterval: ensures a single
        // in-flight poll at a time. With setInterval, a fetch that stalls
        // past the 3s cadence would pile concurrent callbacks on top of
        // each other.
        const pollInterval =
          mode === "local" ? LOCAL_POLL_INTERVAL_MS : POLL_INTERVAL_MS;
        const schedulePoll = (): void => {
          if (cancelled) return;
          pollHandle = setTimeout(async () => {
            if (cancelled) return;
            const controller = new AbortController();
            const timeoutHandle = setTimeout(
              () => controller.abort(),
              REQUEST_TIMEOUT_MS,
            );
            try {
              await runPoll(controller.signal);
            } finally {
              clearTimeout(timeoutHandle);
              schedulePoll();
            }
          }, pollInterval);
        };
        schedulePoll();
      })
      .catch((err) => {
        if (!cancelled) markTransportUnavailable(err);
      });

    return () => {
      cancelled = true;
      if (pollHandle !== undefined) clearTimeout(pollHandle);
    };
    // Intentional: this effect must only re-fire on the five real triggers.
    // All callbacks accessed in the effect body are closures over refs/
    // getState() — they're stable enough that listing them in deps would
    // serve only to spuriously re-init the whole workspace (which is exactly
    // the bug that motivated extracting this hook).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSlug, activeWorkspaceIdentity, mode, port, localReady]);

  // --- Management-route agent refresh --------------------------------------
  //
  // When the user lands on /management, snapshot agents once so the list
  // doesn't lag a poll cycle behind. Gated on workspaceRef so it only fires
  // after init has bound this workspace.
  useEffect(() => {
    const isManagementRoute =
      location.pathname === "/management" ||
      location.pathname.startsWith("/management/");
    if (!isManagementRoute) return;
    if (mode !== "remote" || !activeSlug || !activeWorkspaceIdentity) return;
    if (workspaceRef.current !== activeWorkspaceIdentity) return;

    let cancelled = false;
    fetchAgentSnapshots(activeSlug, activeWorkspaceIdentity)
      .then((snapshot) => {
        if (
          cancelled ||
          activeSlug !== activeSlugRef.current ||
          activeWorkspaceIdentity !== workspaceRef.current
        ) {
          return;
        }
        applyAgentSnapshots(snapshot);
      })
      .catch(() => {
        if (!cancelled) markTransportUnavailable();
      });

    return () => {
      cancelled = true;
    };
    // Intentional: same rationale as the init effect — derived callbacks are
    // stable via refs/getState().
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeSlug, activeWorkspaceIdentity, location.pathname, mode]);
}
