import { useCallback, useEffect, useRef } from "react";
import { Navigate, Route, Routes } from "react-router";
import { ChatLayout } from "./components/chat/chat-layout";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { useAgentStore } from "./hooks/use-agent-store";
import { useChatStore } from "./hooks/use-chat-store";
import type { Agent, Channel, Message, PollChange } from "./lib/types";
import * as client from "./lib/client";
import * as mockClient from "./lib/mock/client";
import { startMockTimer, stopMockTimer } from "./lib/mock/timer";
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
  const setMessages = useChatStore((s) => s.setMessages);
  const incrementUnread = useChatStore((s) => s.incrementUnread);
  const setAgents = useAgentStore((s) => s.setAgents);

  // Mutable refs for poll loop — avoids stale closures
  const sinceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);

  // Keep refs in sync with store
  useEffect(() => {
    return useChatStore.subscribe((state) => {
      currentChannelRef.current = state.currentChannel;
      channelsRef.current = state.channels;
    });
  }, []);

  const runPoll = useCallback(async () => {
    try {
      const pollRes = await mockClient.poll(sinceRef.current);
      if (!pollRes.ok || !pollRes.data) return;

      sinceRef.current = pollRes.data.commit_id as string;
      const changes = (pollRes.data.changes ?? []) as PollChange[];

      let needChannelRefresh = false;

      for (const change of changes) {
        const displayName = apiToDisplay(change.channel);
        const knownChannel = channelsRef.current.some(
          (c) => c.name === displayName
        );

        if (!knownChannel) {
          needChannelRefresh = true;
          continue;
        }

        if (displayName === currentChannelRef.current) {
          // Reload messages for the active channel
          const apiCh = change.channel.startsWith("dm:")
            ? change.channel
            : displayName;
          const readRes = await mockClient.read(apiCh);
          if (readRes.ok && readRes.data) {
            setMessages(readRes.data.entries as Message[]);
          }
        } else {
          incrementUnread(displayName);
        }
      }

      if (needChannelRefresh) {
        const chRes = await mockClient.channels();
        if (chRes.ok && chRes.data) {
          setChannels(chRes.data.channels as Channel[]);
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
  }, [setMessages, incrementUnread, setChannels, setAgents]);

  // Init + poll loop
  useEffect(() => {
    async function init() {
      const [meRes, channelsRes, usersRes, agentsRes] = await Promise.all([
        mockClient.me(),
        mockClient.channels(),
        mockClient.users(),
        client.listAgents(),
      ]);

      if (meRes.ok && meRes.data) setCurrentUser(meRes.data.handler as string);
      if (channelsRes.ok && channelsRes.data)
        setChannels(channelsRes.data.channels as Channel[]);
      if (usersRes.ok && usersRes.data)
        setUsers(usersRes.data.users as string[]);
      if (agentsRes.ok && agentsRes.data)
        setAgents(agentsRes.data.agents as Agent[]);

      setConnected(true);
      startMockTimer();
    }

    init();

    const pollHandle = setInterval(runPoll, POLL_INTERVAL_MS);

    return () => {
      clearInterval(pollHandle);
      stopMockTimer();
    };
  }, [setCurrentUser, setChannels, setUsers, setAgents, setConnected, runPoll]);

  return (
    <SetupGate>
      <Toaster position="top-right" richColors />
      <Routes>
        <Route element={<AppShell />}>
          <Route index element={<Navigate to="/management" replace />} />
          <Route path="/management" element={<ManagementPage />} />
          <Route path="/management/:agentId" element={<AgentDetail />} />
          <Route path="/chat" element={<ChatPage />} />
        </Route>
      </Routes>
    </SetupGate>
  );
}
