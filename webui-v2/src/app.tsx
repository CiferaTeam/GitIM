import { useCallback, useEffect, useRef } from "react";
import { Navigate, Route, Routes } from "react-router";
import { CardDetail } from "./components/cards/card-detail";
import { CardKanban } from "./components/cards/card-kanban";
import { ChatLayout } from "./components/chat/chat-layout";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { useAgentActivitySSE } from "./hooks/use-agent-activity";
import { useVersionCheck } from "./hooks/use-version-check";
import { useAgentStore } from "./hooks/use-agent-store";
import { useCardStore, parseCardScope } from "./hooks/use-card-store";
import { useChatStore } from "./hooks/use-chat-store";
import { useConnectionStore } from "./hooks/use-connection-store";
import type { Agent, Card, Channel, Message, PollChange } from "./lib/types";
import * as client from "./lib/client";
import { loadCursor, saveCursor, clearCursor } from "./lib/cursor";
import { SetupGate } from "./components/setup/setup-gate";
import { Toaster } from "sonner";

const POLL_INTERVAL_MS = 3000;

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

export default function App() {
  const setCurrentUser = useChatStore((s) => s.setCurrentUser);
  const setChannels = useChatStore((s) => s.setChannels);
  const setUsers = useChatStore((s) => s.setUsers);
  const setConnected = useChatStore((s) => s.setConnected);
  const addMessages = useChatStore((s) => s.addMessages);
  const incrementUnread = useChatStore((s) => s.incrementUnread);
  const setAgents = useAgentStore((s) => s.setAgents);
  const setCards = useCardStore((s) => s.setCards);
  const mergeCards = useCardStore((s) => s.mergeCards);
  const addCardMessages = useCardStore((s) => s.addCardMessages);
  const port = useConnectionStore((s) => s.port);

  // Mutable refs for poll loop — avoids stale closures
  const sinceRef = useRef<string | undefined>(undefined);
  const workspaceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);

  useVersionCheck();
  // Connect to agent activity SSE stream
  useAgentActivitySSE();

  // Keep refs in sync with store
  useEffect(() => {
    return useChatStore.subscribe((state) => {
      currentChannelRef.current = state.currentChannel;
      channelsRef.current = state.channels;
    });
  }, []);

  const runPoll = useCallback(async () => {
    try {
      const pollRes = await client.poll(sinceRef.current);

      if (!pollRes.ok || !pollRes.data) {
        // Stale cursor recovery: discard and re-init
        if (pollRes.error && workspaceRef.current) {
          clearCursor(workspaceRef.current);
          sinceRef.current = undefined;
        }
        return;
      }

      sinceRef.current = pollRes.data.commit_id as string;
      if (workspaceRef.current) {
        saveCursor(workspaceRef.current, sinceRef.current);
      }

      const changes = (pollRes.data.changes ?? []) as PollChange[];

      let needChannelRefresh = false;
      let needCardRefresh = false;

      for (const change of changes) {
        // Card events: channel string is "card:<channel>/<card_id>"
        if (change.kind === "card_meta" || change.kind === "card_thread") {
          needCardRefresh = true;
          if (change.kind === "card_thread" && change.entries?.length) {
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
          if (!knownChannel) continue;
        }

        if (displayName === currentChannelRef.current) {
          if (change.entries?.length) {
            addMessages(change.entries as Message[]);
          }
        } else {
          const me = useChatStore.getState().currentUser;
          const mentionTag = `<@${me}>`;
          const mentioned = !!change.entries?.some((e) =>
            (e as Message).body?.includes(mentionTag)
          );
          incrementUnread(displayName, mentioned);
        }
      }

      if (needChannelRefresh) {
        const chRes = await client.channels();
        if (chRes.ok && chRes.data) {
          setChannels(chRes.data.channels as Channel[]);
        }
      }

      if (needCardRefresh) {
        const cardRes = await client.listCards();
        if (cardRes.ok && cardRes.data) {
          // Merge, not replace — preserves in-flight optimistic patches so
          // the 3s poll cadence can't flicker the UI back before PATCH resolves.
          mergeCards(cardRes.data.cards as Card[]);
        }
      }

      // Periodically refresh agents (real backend)
      const agentsRes = await client.listAgents();
      if (agentsRes.ok && agentsRes.data) {
        setAgents(agentsRes.data.agents as Agent[]);
      }
    } catch {
      // Silently skip failed polls
    }
  }, [
    addMessages,
    incrementUnread,
    setChannels,
    setAgents,
    mergeCards,
    addCardMessages,
  ]);

  // Init + poll loop — only run when port is available
  useEffect(() => {
    if (!port) return;

    async function init() {
      const [healthRes, meRes, channelsRes, usersRes, agentsRes, cardsRes] =
        await Promise.all([
          client.health(),
          client.me(),
          client.channels(),
          client.users(),
          client.listAgents(),
          client.listCards(),
        ]);

      // Restore cursor from localStorage keyed by workspace
      if (healthRes.ok && healthRes.data?.workspace) {
        workspaceRef.current = healthRes.data.workspace as string;
        sinceRef.current = loadCursor(workspaceRef.current);
      }

      if (meRes.ok && meRes.data) setCurrentUser(meRes.data.handler as string);
      if (channelsRes.ok && channelsRes.data)
        setChannels(channelsRes.data.channels as Channel[]);
      if (usersRes.ok && usersRes.data)
        setUsers(usersRes.data.users as string[]);
      if (agentsRes.ok && agentsRes.data)
        setAgents(agentsRes.data.agents as Agent[]);
      if (cardsRes.ok && cardsRes.data)
        setCards(cardsRes.data.cards as Card[]);

      setConnected(true);
    }

    let pollHandle: ReturnType<typeof setInterval>;
    init().then(() => {
      pollHandle = setInterval(runPoll, POLL_INTERVAL_MS);
    });

    return () => {
      clearInterval(pollHandle);
    };
  }, [port, setCurrentUser, setChannels, setUsers, setAgents, setCards, setConnected, runPoll]);

  return (
    <SetupGate>
      <Toaster position="top-right" richColors />
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/management" replace />} />
          <Route path="/management" element={<ManagementPage />} />
          <Route path="/management/:agentId" element={<AgentDetail />} />
          <Route path="/chat" element={<ChatPage />} />
          <Route path="/cards" element={<CardKanban />} />
          <Route path="/cards/:channel/:card_id" element={<CardDetail />} />
        </Route>
      </Routes>
    </SetupGate>
  );
}
