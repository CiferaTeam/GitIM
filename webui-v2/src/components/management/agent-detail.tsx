import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as client from "@/lib/client";
import type { Agent } from "@/lib/types";
import { ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { relativeTime, statusBadge } from "./agent-card";
import { RemoveAgentDialog } from "./remove-agent-dialog";
import { useState } from "react";
import { toast } from "sonner";

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

  const activities = useAgentActivityStore((s) => s.activities);

  const agent: Agent | undefined = agents.find((a) => a.id === agentId);
  const agentEvents = agent ? (activities[agent.id] ?? []) : [];

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
      const res = await client.stopAgent(agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to stop agent");
      }
    } else {
      const res = await client.startAgent(agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to start agent");
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

        <Field label="Model">
          <code className="text-sm font-mono text-muted-foreground">
            {agent.model ?? "claude-sonnet-4-6"}
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

      {/* Environment Variables */}
      {agent.env && Object.keys(agent.env).length > 0 && (
        <div className="mb-6">
          <Field label="Environment Variables">
            <div className="mt-1 border rounded-md p-3 bg-muted/30 space-y-1">
              {Object.entries(agent.env).map(([key, value]) => (
                <div key={key} className="text-sm font-mono">
                  <span className="text-muted-foreground">{key}</span>
                  <span className="text-muted-foreground mx-1">=</span>
                  <span>{value}</span>
                </div>
              ))}
            </div>
          </Field>
        </div>
      )}

      {/* Activity Log */}
      <div className="mb-6">
        <p className="text-xs text-muted-foreground font-medium uppercase tracking-wide mb-2">
          Activity Log
        </p>
        <ScrollArea className="h-48 border rounded-md">
          <div className="p-3 space-y-1">
            {agentEvents.length === 0 ? (
              <p className="text-sm text-muted-foreground">No activity yet</p>
            ) : (
              agentEvents.map((ev, i) => (
                <p key={i} className="text-sm font-mono">
                  <span className="text-muted-foreground">
                    {ev.timestamp.slice(11, 16)}
                  </span>
                  {" — "}
                  <span className="text-muted-foreground">{ev.event_type}</span>
                  {" "}
                  {ev.detail}
                </p>
              ))
            )}
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
