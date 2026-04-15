import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardFooter,
  CardHeader,
} from "@/components/ui/card";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as client from "@/lib/client";
import type { Agent, AgentStatus } from "@/lib/types";
import { useState } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { RemoveAgentDialog } from "./remove-agent-dialog";

export function relativeTime(isoString: string): string {
  const diff = Math.floor((Date.now() - new Date(isoString).getTime()) / 1000);
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)} min ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)} hours ago`;
  return `${Math.floor(diff / 86400)} days ago`;
}

export function statusBadge(status: AgentStatus) {
  switch (status) {
    case "running":
      return (
        <Badge className="bg-success text-white hover:bg-success">
          Running
        </Badge>
      );
    case "idle":
      return (
        <Badge className="bg-warning text-white hover:bg-warning">
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
  const navigate = useNavigate();
  const [removeOpen, setRemoveOpen] = useState(false);

  const isRunning = agent.status !== "offline";

  async function handleToggle() {
    if (isRunning) {
      const res = await client.stopAgent(agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to stop agent");
      }
    } else {
      const res = await client.startAgent(agent.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to start agent");
      }
    }
  }

  return (
    <>
      <Card
        className="cursor-pointer hover:shadow-md transition-all duration-150 hover:border-border/80 bg-card/50"
        onClick={() => navigate(`/management/${agent.id}`)}
      >
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
            onClick={() => navigate(`/management/${agent.id}`)}
          >
            Details
          </Button>
          <Button
            variant="destructive"
            size="sm"
            onClick={() => setRemoveOpen(true)}
          >
            Remove
          </Button>
        </CardFooter>
      </Card>

      <RemoveAgentDialog
        agentId={agent.id}
        agentName={agent.name}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
      />
    </>
  );
}
