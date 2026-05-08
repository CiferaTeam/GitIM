import { useCallback, useEffect, useRef } from "react";
import { Navigate, Route, Routes, useLocation } from "react-router";
import { Loader2 } from "lucide-react";
import { CardDetail } from "./components/cards/card-detail";
import { CardKanban } from "./components/cards/card-kanban";
import { ChatLayout } from "./components/chat/chat-layout";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { DocsPage } from "./components/docs/docs-page";
import { useAgentActivitySSE } from "./hooks/use-agent-activity";
import { useAgentStore } from "./hooks/use-agent-store";
import { useCardStore, parseCardScope } from "./hooks/use-card-store";
import { useChatStore } from "./hooks/use-chat-store";
import { useConnectionStore } from "./hooks/use-connection-store";
import { useIsMobile } from "./hooks/use-media-query";
import { useWorkspaceStore } from "./hooks/use-workspace-store";
import type { Agent, Card, Channel, Message, PollChange } from "./lib/types";
import * as client from "./lib/client";
import { loadCursor, saveCursor, clearCursor } from "./lib/cursor";
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
  const incrementUnread = useChatStore((s) => s.incrementUnread);
  const resetChatForSwitch = useChatStore((s) => s.resetForWorkspaceSwitch);
  const setAgents = useAgentStore((s) => s.setAgents);
  const resetAgentsForSwitch = useAgentStore((s) => s.resetForWorkspaceSwitch);
  const setCards = useCardStore((s) => s.setCards);
  const mergeCards = useCardStore((s) => s.mergeCards);
  const addCardMessages = useCardStore((s) => s.addCardMessages);
  const resetCardsForSwitch = useCardStore((s) => s.resetForWorkspaceSwitch);
  const port = useConnectionStore((s) => s.port);
  const mode = useConnectionStore((s) => s.mode);
  const localReady = useConnectionStore((s) => s.localReady);
  const setHeadCommit = useConnectionStore((s) => s.setHeadCommit);
  const setConnectionStatus = useConnectionStore((s) => s.setStatus);
  const setConnectionError = useConnectionStore((s) => s.setError);
  const isMobile = useIsMobile();

  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspacesLoading = useWorkspaceStore((s) => s.loading);
  const fetchWorkspaces = useWorkspaceStore((s) => s.fetchAll);

  // Mutable refs for poll loop — avoids stale closures
  const sinceRef = useRef<string | undefined>(undefined);
  const workspaceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);
  const activeSlugRef = useRef<string | null>(null);

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

  const runPoll = useCallback(async (signal?: AbortSignal) => {
    const slug = activeSlugRef.current;
    if (!slug) return;
    try {
      const pollRes = await client.poll(slug, sinceRef.current, signal);

      if (!pollRes.ok || !pollRes.data) {
        // Stale cursor recovery: discard and re-init
        if (pollRes.error && workspaceRef.current) {
          clearCursor(workspaceRef.current);
          sinceRef.current = undefined;
        }
        markWorkspaceUnavailable();
        return;
      }

      sinceRef.current = pollRes.data.commit_id as string;
      if (workspaceRef.current) {
        saveCursor(workspaceRef.current, sinceRef.current);
      }
      setHeadCommit(sinceRef.current);

      markConnected();

      const changes = (pollRes.data.changes ?? []) as PollChange[];

      let needChannelRefresh = false;
      let needArchivedRefresh = false;
      let needCardRefresh = false;

      for (const change of changes) {
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
        if (chRes.ok && chRes.data) {
          setChannels(chRes.data.channels as Channel[]);
        }
      }

      if (needArchivedRefresh) {
        const arRes = await client.listArchivedChannels(slug);
        if (arRes.ok && arRes.data) {
          setArchivedChannels(arRes.data.channels as Channel[]);
        }
      }

      if (needCardRefresh) {
        const cardRes = await client.listCards(slug);
        if (cardRes.ok && cardRes.data) {
          // Merge, not replace — preserves in-flight optimistic patches so
          // the 3s poll cadence can't flicker the UI back before PATCH resolves.
          mergeCards(cardRes.data.cards as Card[]);
        }
      }

      if (mode === "remote") {
        const agentsRes = await client.listAgents(slug);
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
    mergeCards,
    addCardMessages,
    setHeadCommit,
    markConnected,
    markWorkspaceUnavailable,
    markTransportUnavailable,
    mode,
  ]);

  // Init + poll loop — runs whenever port + activeSlug are both set, and
  // re-runs whenever activeSlug changes so state is refreshed on switch.
  useEffect(() => {
    if (!activeSlug) return;
    if (mode === "remote" && !port) return;
    if (mode === "local" && !localReady) return;
    const activeWorkspace = workspaces.find((w) => w.slug === activeSlug);
    if (!activeWorkspace) return;
    const workspaceKey = workspaceIdentity(mode, activeWorkspace);

    // Reset per-workspace store slices on switch so stale data from the
    // previous workspace doesn't leak into the new one. Each store owns
    // the knowledge of which of its fields are workspace-scoped — in
    // particular chat resets `currentChannel` + `messages` so poll-driven
    // `addMessages` can't append ws-B entries onto ws-A's list.
    resetChatForSwitch();
    resetAgentsForSwitch();
    resetCardsForSwitch();
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

    async function init(slug: string) {
      if (mode === "local") {
        const activation = await client.activateBrowserWorkspace(slug, {
          onSyncReset: () => {
            clearCursor(workspaceKey);
            sinceRef.current = undefined;
            resetChatForSwitch();
            resetAgentsForSwitch();
            resetCardsForSwitch();
          },
        });
        if (cancelled) return;
        if (activation.error_code === "activation_superseded") return;
        if (!activation.ok) {
          setConnectionError(activation.error ?? "Failed to activate browser workspace");
          return;
        }
      }

      const [meRes, channelsRes, usersRes, agentsRes, cardsRes] =
        await Promise.all([
          client.me(slug),
          client.channels(slug),
          client.users(slug),
          mode === "remote"
            ? client.listAgents(slug)
            : Promise.resolve({ ok: true, data: { agents: [] } }),
          client.listCards(slug),
        ]);

      if (cancelled) return;

      // Restore cursor from localStorage keyed by runtime or browser workspace identity.
      workspaceRef.current = workspaceKey;
      sinceRef.current = loadCursor(workspaceKey);

      if (meRes.ok && meRes.data) setCurrentUser(meRes.data.handler as string);
      if (channelsRes.ok && channelsRes.data)
        setChannels(channelsRes.data.channels as Channel[]);
      if (usersRes.ok && usersRes.data)
        setUsers(usersRes.data.users as string[]);
      if (agentsRes.ok && agentsRes.data)
        setAgents(agentsRes.data.agents as Agent[]);
      if (cardsRes.ok && cardsRes.data)
        setCards(cardsRes.data.cards as Card[]);

      const bootstrapOk =
        meRes.ok &&
        channelsRes.ok &&
        usersRes.ok &&
        agentsRes.ok &&
        cardsRes.ok;
      if (bootstrapOk) {
        markConnected();
      }
    }

    init(activeSlug).then(() => {
      if (cancelled) return;

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
    workspaces,
    setCurrentUser,
    setChannels,
    setUsers,
    setAgents,
    setCards,
    setHeadCommit,
    resetChatForSwitch,
    resetAgentsForSwitch,
    resetCardsForSwitch,
    setConnectionError,
    markConnected,
    runPoll,
  ]);

  // /docs is a standalone reference — let it render regardless of workspace
  // state so setup-screen hints ("What scopes does the PAT need?") can deep-link
  // into it without getting bounced back by the gate.
  const location = useLocation();
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
            <Route path="/chat" element={<ChatPage />} />
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
