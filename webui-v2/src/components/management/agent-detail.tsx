import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import * as client from "@/lib/client";
import type { Agent } from "@/lib/types";
import { ArrowLeft, Play, Pause, Trash2 } from "lucide-react";
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
    <div className="space-y-1.5">
      <p className="text-xs text-text-muted font-semibold uppercase tracking-wider">
        {label}
      </p>
      <div className="text-sm">{children}</div>
    </div>
  );
}

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

function avatarColor(name: string) {
  const hues = [210, 150, 30, 280, 340, 190, 45, 260];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = name.charCodeAt(i) + ((hash << 5) - hash);
  const hue = hues[Math.abs(hash) % hues.length];
  return `hsl(${hue} 70% 55%)`;
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
        <p className="mt-4 text-text-muted">Agent not found.</p>
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
        className="mb-4 text-text-secondary hover:text-foreground"
        onClick={() => navigate("/management")}
      >
        <ArrowLeft className="size-4 mr-1" />
        Back
      </Button>

      {/* Header */}
      <div className="flex items-start gap-4 mb-8">
        <div
          className="w-16 h-16 rounded-2xl flex items-center justify-center text-xl font-bold text-white shadow-lg"
          style={{ backgroundColor: avatarColor(agent.name || agent.id) }}
        >
          {initials(agent.name || agent.id)}
        </div>
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-3 flex-wrap">
            <h1 className="text-2xl font-semibold tracking-tight">{agent.name}</h1>
            {statusBadge(agent.status)}
          </div>
          <p className="text-sm text-text-muted mt-1 font-mono truncate">
            {agent.id}
          </p>
        </div>
      </div>

      {/* Info grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-8 p-5 rounded-xl border border-border bg-card/50">
        <Field label="Repo Path">
          <code className="text-sm font-mono text-text-secondary bg-background/60 px-2 py-1 rounded">
            {agent.repoPath}
          </code>
        </Field>

        <Field label="Model">
          <span className="inline-flex items-center px-2 py-0.5 rounded bg-background/60 border border-border text-sm font-mono">
            {agent.model ?? "claude-sonnet-4-6"}
          </span>
        </Field>

        <Field label="Messages Processed">
          <span className="text-lg font-semibold">{agent.messagesProcessed}</span>
        </Field>

        <Field label="Last Activity">
          <span className="text-text-secondary">
            {agent.lastActivity ? relativeTime(agent.lastActivity) : "—"}
          </span>
        </Field>
      </div>

      {/* System Prompt */}
      <div className="mb-8">
        <Field label="System Prompt">
          <div className="mt-2 rounded-xl border border-border bg-card/50 p-4">
            <pre className="text-sm whitespace-pre-wrap font-mono break-words text-text-secondary leading-relaxed">
              {agent.systemPrompt || "(none)"}
            </pre>
          </div>
        </Field>
      </div>

      {/* Environment Variables */}
      {agent.env && Object.keys(agent.env).length > 0 && (
        <div className="mb-8">
          <Field label="Environment Variables">
            <div className="mt-2 rounded-xl border border-border bg-card/50 p-4 space-y-2">
              {Object.entries(agent.env).map(([key, value]) => (
                <div key={key} className="text-sm font-mono flex items-center gap-2">
                  <span className="text-primary font-medium">{key}</span>
                  <span className="text-text-muted">=</span>
                  <span className="text-text-secondary">{value}</span>
                </div>
              ))}
            </div>
          </Field>
        </div>
      )}

      {/* Activity Log */}
      <div className="mb-8">
        <p className="text-xs text-text-muted font-semibold uppercase tracking-wider mb-3">
          Activity Log
        </p>
        <ScrollArea className="h-56 rounded-xl border border-border bg-card/50">
          <div className="p-4 space-y-2">
            {agentEvents.length === 0 ? (
              <p className="text-sm text-text-muted">No activity yet</p>
            ) : (
              agentEvents.map((ev, i) => (
                <div key={i} className="flex items-start gap-3 text-sm">
                  <span className="text-text-faint shrink-0 font-mono text-xs pt-0.5">
                    {ev.timestamp.slice(11, 16)}
                  </span>
                  <div className="flex-1">
                    <span className="inline-block px-1.5 py-0.5 rounded text-[10px] font-medium uppercase tracking-wide bg-surface text-text-muted mb-0.5">
                      {ev.event_type}
                    </span>
                    <p className="text-text-secondary">{ev.detail}</p>
                  </div>
                </div>
              ))
            )}
          </div>
        </ScrollArea>
      </div>

      {/* Actions */}
      <div className="flex gap-3">
        <Button
          variant={isRunning ? "outline" : "default"}
          size="default"
          onClick={handleToggle}
          className={isRunning ? "border-border-strong hover:bg-surface-hover" : ""}
        >
          {isRunning ? (
            <><Pause className="size-4 mr-1.5" /> Stop</>
          ) : (
            <><Play className="size-4 mr-1.5" /> Start</>
          )}
        </Button>
        <Button
          variant="ghost"
          size="default"
          onClick={() => setRemoveOpen(true)}
          className="text-destructive hover:text-destructive hover:bg-destructive/10"
        >
          <Trash2 className="size-4 mr-1.5" />
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
