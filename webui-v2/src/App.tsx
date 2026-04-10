import { useEffect } from "react";
import { Navigate, Route, Routes } from "react-router";
import { ChatLayout } from "./components/chat/chat-layout";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { useAgentStore } from "./hooks/use-agent-store";
import { useChatStore } from "./hooks/use-chat-store";
import type { Agent, Channel } from "./lib/types";
import * as mockClient from "./lib/mock/client";
import { startMockTimer, stopMockTimer } from "./lib/mock/timer";

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
  const setAgents = useAgentStore((s) => s.setAgents);

  useEffect(() => {
    async function init() {
      const [meRes, channelsRes, usersRes, agentsRes] = await Promise.all([
        mockClient.me(),
        mockClient.channels(),
        mockClient.users(),
        mockClient.listAgents(),
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

    return () => {
      stopMockTimer();
    };
  }, [setCurrentUser, setChannels, setUsers, setAgents, setConnected]);

  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route index element={<Navigate to="/management" replace />} />
        <Route path="/management" element={<ManagementPage />} />
        <Route path="/management/:agentId" element={<AgentDetail />} />
        <Route path="/chat" element={<ChatPage />} />
      </Route>
    </Routes>
  );
}
