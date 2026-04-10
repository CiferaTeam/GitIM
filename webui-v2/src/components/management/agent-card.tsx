import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from "@/components/ui/card";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as mockClient from "@/lib/mock/client";
import type { Agent, AgentStatus } from "@/lib/types";

export function relativeTime(isoString: string): string {
  const diff = Math.floor((Date.now() - new Date(isoString).getTime()) / 1000);
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} hours ago`;
  return `${Math.floor(diff / 86400)} days ago`;
}

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

interface AgentCardProps {
  agent: Agent;
}

export function AgentCard({ agent }: AgentCardProps) {
  const updateAgent = useAgentStore((s) => s.updateAgent);
  const removeAgent = useAgentStore((s) => s.removeAgent);

  const isRunning = agent.status !== "offline";

  async function handleToggle() {
    if (isRunning) {
      const res = await mockClient.stopAgent(agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      }
    } else {
      const res = await mockClient.startAgent(agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      }
    }
  }

  async function handleRemove() {
    const res = await mockClient.removeAgent(agent.id);
    if (res.ok) {
      removeAgent(agent.id);
    }
  }

  return (
    <Card className="cursor-pointer hover:shadow-md transition-shadow" onClick={() => {}}>
      <CardHeader className="pb-2">
        <div className="flex items-center justify-between gap-2">
          <span className="font-semibold text-lg truncate">{agent.name}</span>
          {statusBadge(agent.status)}
        </div>
      </CardHeader>

      <CardContent>
        <div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-1.5 text-sm">
          <span className="text-muted-foreground">Last activity</span>
          <span>
            {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
          </span>

          <span className="text-muted-foreground">Messages processed</span>
          <span>{agent.messagesProcessed}</span>

          <span className="text-muted-foreground">Current channel</span>
          <span>{agent.currentChannel ?? "—"}</span>
        </div>
      </CardContent>

      <CardFooter className="gap-2 flex-wrap" onClick={(e) => e.stopPropagation()}>
        <Button
          variant="outline"
          size="sm"
          onClick={handleToggle}
        >
          {isRunning ? "Stop" : "Start"}
        </Button>
        <Button
          variant="outline"
          size="sm"
          onClick={() => {}}
        >
          Details
        </Button>
        <Button
          variant="destructive"
          size="sm"
          onClick={handleRemove}
        >
          Remove
        </Button>
      </CardFooter>
    </Card>
  );
}
