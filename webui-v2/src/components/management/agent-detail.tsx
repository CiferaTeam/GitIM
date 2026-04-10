import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as mockClient from "@/lib/mock/client";
import type { Agent, AgentStatus } from "@/lib/types";
import { ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { relativeTime } from "./agent-card";
import { RemoveAgentDialog } from "./remove-agent-dialog";
import { useState } from "react";

const MOCK_LOG = [
  { time: "10:23", text: "Received message in #dev-tasks" },
  { time: "10:24", text: "Spawned sub-agent for code review" },
  { time: "10:25", text: "Completed review, posted results" },
  { time: "10:27", text: "Waiting for new messages" },
  { time: "10:31", text: "Received message in #general" },
  { time: "10:31", text: "Replied with summary of recent commits" },
  { time: "10:45", text: "Started scheduled task: daily standup digest" },
  { time: "10:46", text: "Standup digest posted to #standup" },
];

function statusBadge(status: AgentStatus) {
  switch (status) {
    case "running":
      return (
        <Badge className="bg-green-500 text-white hover:bg-green-500">
          Running
        </Badge>
      );
    case "idle":
      return (
        <Badge className="bg-amber-400 text-white hover:bg-amber-400">
          Idle
        </Badge>
      );
    case "error":
      return <Badge variant="destructive">Error</Badge>;
    case "offline":
      return <Badge variant="secondary">Offline</Badge>;
  }
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div className="space-y-1">
      <p className="text-xs text-muted-foreground font-medium uppercase tracking-wide">
        {label}
      </p>
      <div>{children}</div>
    </div>
  );
}

export function AgentDetail() {
  const { agentId } = useParams<{ agentId: string }>();
  const navigate = useNavigate();
  const agents = useAgentStore((s) => s.agents);
  const updateAgent = useAgentStore((s) => s.updateAgent);
  const [removeOpen, setRemoveOpen] = useState(false);

  const agent: Agent | undefined = agents.find((a) => a.id === agentId);

  if (!agent) {
    return (
      <div className="p-6">
        <Button variant="ghost" size="sm" onClick={() => navigate("/management")}>
          <ArrowLeft className="size-4 mr-1" />
          Back
        </Button>
        <p className="mt-4 text-muted-foreground">Agent not found.</p>
      </div>
    );
  }

  const isRunning = agent.status !== "offline";

  async function handleToggle() {
    if (isRunning) {
      const res = await mockClient.stopAgent(agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      }
    } else {
      const res = await mockClient.startAgent(agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      }
    }
  }

  return (
    <div className="p-6 max-w-3xl">
      <Button
        variant="ghost"
        size="sm"
        className="mb-4"
        onClick={() => navigate("/management")}
      >
        <ArrowLeft className="size-4 mr-1" />
        Back
      </Button>

      {/* Header */}
      <div className="flex items-center gap-3 mb-6">
        <h1 className="text-2xl font-semibold">{agent.name}</h1>
        {statusBadge(agent.status)}
      </div>

      {/* Fields */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-6">
        <Field label="ID">
          <code className="text-sm font-mono text-muted-foreground">
            {agent.id}
          </code>
        </Field>

        <Field label="Repo Path">
          <code className="text-sm font-mono text-muted-foreground">
            {agent.repoPath}
          </code>
        </Field>

        <Field label="Session ID">
          <code className="text-sm font-mono text-muted-foreground">
            {agent.sessionId ?? "—"}
          </code>
        </Field>

        <Field label="Messages Processed">
          <span className="text-sm">{agent.messagesProcessed}</span>
        </Field>

        <Field label="Last Activity">
          <span className="text-sm">
            {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
          </span>
        </Field>

        <Field label="Current Channel">
          <span className="text-sm">{agent.currentChannel ?? "—"}</span>
        </Field>
      </div>

      {/* System Prompt */}
      <div className="mb-6">
        <Field label="System Prompt">
          <div className="mt-1 border rounded-md p-3 bg-muted/30">
            <pre className="text-sm whitespace-pre-wrap font-mono break-words">
              {agent.systemPrompt || "(none)"}
            </pre>
          </div>
        </Field>
      </div>

      {/* Activity Log */}
      <div className="mb-6">
        <p className="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-2">
          Activity Log
        </p>
        <ScrollArea className="h-48 border rounded-md">
          <div className="p-3 space-y-1">
            {MOCK_LOG.map((entry, i) => (
              <p key={i} className="text-sm font-mono">
                <span className="text-muted-foreground">{entry.time}</span>
                {" — "}
                {entry.text}
              </p>
            ))}
          </div>
        </ScrollArea>
      </div>

      {/* Actions */}
      <div className="flex gap-2">
        <Button variant="outline" onClick={handleToggle}>
          {isRunning ? "Stop" : "Start"}
        </Button>
        <Button variant="destructive" onClick={() => setRemoveOpen(true)}>
          Remove
        </Button>
      </div>

      <RemoveAgentDialog
        agentId={agent.id}
        agentName={agent.name}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
        onRemoved={() => navigate("/management")}
      />
    </div>
  );
}
