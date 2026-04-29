import { useCallback, useEffect, useRef } from "react";
import { Navigate, Route, Routes } from "react-router";
import { ChatLayout } from "./components/chat/chat-layout";
import { AppShell } from "./components/layout/app-shell";
import { AgentDetail } from "./components/management/agent-detail";
import { AgentList } from "./components/management/agent-list";
import { useAgentActivitySSE } from "./hooks/use-agent-activity";
import { useAgentStore } from "./hooks/use-agent-store";
import { useChatStore } from "./hooks/use-chat-store";
import { useConnectionStore } from "./hooks/use-connection-store";
import type { Agent, Channel, Message, PollChange } from "./lib/types";
import * as client from "./lib/client";
import { loadCursor, saveCursor, clearCursor } from "./lib/cursor";
import { SetupGate } from "./components/setup/setup-gate";
import { InviteGate } from "./components/invite/invite-gate";
import { Toaster } from "sonner";

const REMOTE_POLL_MS = 3000;
const LOCAL_POLL_MS = 7000;

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
  const mode = useConnectionStore((s) => s.mode);
  const status = useConnectionStore((s) => s.status);
  const localReady = useConnectionStore((s) => s.localReady);
  const setCurrentUser = useChatStore((s) => s.setCurrentUser);
  const setChannels = useChatStore((s) => s.setChannels);
  const setUsers = useChatStore((s) => s.setUsers);
  const setConnected = useChatStore((s) => s.setConnected);
  const addMessages = useChatStore((s) => s.addMessages);
  const incrementUnread = useChatStore((s) => s.incrementUnread);
  const setMessages = useChatStore((s) => s.setMessages);
  const setAgents = useAgentStore((s) => s.setAgents);

  // Mutable refs for poll loop — avoids stale closures
  const sinceRef = useRef<string | undefined>(undefined);
  const workspaceRef = useRef<string | undefined>(undefined);
  const currentChannelRef = useRef<string | null>(null);
  const channelsRef = useRef<Channel[]>([]);

  // Connect to agent activity SSE stream (remote mode only)
  useAgentActivitySSE(mode === "remote");

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

      for (const change of changes) {
        const displayName = apiToDisplay(change.channel);
        const knownChannel = channelsRef.current.some(
          (c) => c.name === displayName,
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
          incrementUnread(displayName);
        }
      }

      if (needChannelRefresh) {
        const chRes = await client.channels();
        if (chRes.ok && chRes.data) {
          setChannels(chRes.data.channels as Channel[]);
        }
      }

      // Refresh agents (remote mode only)
      if (mode === "remote") {
        const agentsRes = await client.listAgents();
        if (agentsRes.ok && agentsRes.data) {
          setAgents(agentsRes.data.agents as Agent[]);
        }
      }
    } catch {
      // Silently skip failed polls
    }
  }, [addMessages, incrementUnread, setChannels, setAgents, mode]);

  // Init + poll loop
  useEffect(() => {
    const backendReady =
      mode === "local" ? localReady : status === "ready";
    if (!backendReady) return;

    async function init() {
      const isLocal = mode === "local";
      const pollInterval = isLocal ? LOCAL_POLL_MS : REMOTE_POLL_MS;

      const initPromises: Promise<unknown>[] = [
        client.health(),
        client.me(),
        client.channels(),
        client.users(),
      ];
      // Only fetch agents in remote mode
      if (!isLocal) {
        initPromises.push(client.listAgents());
      }

      const results = await Promise.all(initPromises);
      const [healthRes, meRes, channelsRes, usersRes] = results as [
        typeof results[0],
        typeof results[0],
        typeof results[0],
        typeof results[0],
      ];

      // Restore cursor
      // Local mode uses "local" as workspace key; remote uses the actual workspace path
      const healthData = (healthRes as { ok: boolean; data?: Record<string, unknown> });
      if (healthData.ok && healthData.data?.workspace) {
        workspaceRef.current = healthData.data.workspace as string;
        sinceRef.current = loadCursor(workspaceRef.current);
      } else if (isLocal) {
        workspaceRef.current = "local";
        sinceRef.current = loadCursor("local");
      }

      const meData = meRes as { ok: boolean; data?: Record<string, unknown> };
      if (meData.ok && meData.data)
        setCurrentUser(meData.data.handler as string);

      const chData = channelsRes as { ok: boolean; data?: Record<string, unknown> };
      if (chData.ok && chData.data)
        setChannels(chData.data.channels as Channel[]);

      const uData = usersRes as { ok: boolean; data?: Record<string, unknown> };
      if (uData.ok && uData.data)
        setUsers(uData.data.users as string[]);

      if (!isLocal && results[4]) {
        const agentsRes = results[4] as { ok: boolean; data?: Record<string, unknown> };
        if (agentsRes.ok && agentsRes.data)
          setAgents(agentsRes.data.agents as Agent[]);
      }

      setConnected(true);
      return pollInterval;
    }

    let pollHandle: ReturnType<typeof setInterval>;
    init().then((interval) => {
      pollHandle = setInterval(runPoll, interval);
    });

    return () => {
      clearInterval(pollHandle);
    };
  }, [
    mode,
    status,
    localReady,
    setCurrentUser,
    setChannels,
    setUsers,
    setAgents,
    setConnected,
    runPoll,
  ]);

  // Listen for sync_reset from LocalBackend — trigger full message reload
  useEffect(() => {
    if (mode !== "local") return;
    // The LocalBackend calls onSyncReset which we handle here by reloading current channel
    const handleSyncReset = async () => {
      sinceRef.current = undefined;
      if (workspaceRef.current) clearCursor(workspaceRef.current);
      // Reload current channel messages
      if (currentChannelRef.current) {
        const res = await client.read(currentChannelRef.current);
        if (res.ok && res.data) {
          setMessages(res.data.entries as Message[]);
        }
      }
      // Refresh channels
      const chRes = await client.channels();
      if (chRes.ok && chRes.data) {
        setChannels(chRes.data.channels as Channel[]);
      }
    };

    // Store the handler globally so LocalBackend can call it
    (window as unknown as Record<string, unknown>).__gitimSyncReset =
      handleSyncReset;
    return () => {
      delete (window as unknown as Record<string, unknown>).__gitimSyncReset;
    };
  }, [mode, setMessages, setChannels]);

  const isLocal = mode === "local";

  return (
    <InviteGate>
      <SetupGate>
        <Toaster position="top-right" richColors />
        <Routes>
          <Route element={<AppShell />}>
            <Route
              index
              element={
                <Navigate to={isLocal ? "/chat" : "/management"} replace />
              }
            />
            {!isLocal && (
              <>
                <Route path="/management" element={<ManagementPage />} />
                <Route
                  path="/management/:agentId"
                  element={<AgentDetail />}
                />
              </>
            )}
            <Route path="/chat" element={<ChatPage />} />
          </Route>
        </Routes>
      </SetupGate>
    </InviteGate>
  );
}
