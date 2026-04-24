import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { Textarea } from "@/components/ui/textarea";
import { useAgentActivityStore } from "@/hooks/use-agent-activity";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { PROVIDERS } from "@/lib/providers";
import type { Agent } from "@/lib/types";
import { ArrowLeft, Play, Pause, Trash2, Pencil } from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { relativeTime, statusBadge } from "./agent-card";
import { ProviderBadge } from "./provider-badge";
import { RemoveAgentDialog } from "./remove-agent-dialog";
import { EnvVarsEditor, type EnvVar } from "./env-vars-editor";
import { useState, useEffect } from "react";
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
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const agents = useAgentStore((s) => s.agents);
  const updateAgent = useAgentStore((s) => s.updateAgent);
  const [removeOpen, setRemoveOpen] = useState(false);

  type Mode = "view" | "edit" | "saving";
  const [mode, setMode] = useState<Mode>("view");
  const [draftModel, setDraftModel] = useState("");
  const [draftPrompt, setDraftPrompt] = useState("");
  const [draftEnv, setDraftEnv] = useState<EnvVar[]>([]);
  const [editError, setEditError] = useState<string | null>(null);

  const activities = useAgentActivityStore((s) => s.activities);

  const agent: Agent | undefined = agents.find((a) => a.id === agentId);
  const agentEvents = agent ? (activities[agent.id] ?? []) : [];

  function enterEditMode() {
    if (!agent) return;
    setDraftModel(agent.model ?? "");
    setDraftPrompt(agent.systemPrompt ?? "");
    setDraftEnv(
      Object.entries(agent.env ?? {}).map(([key, value]) => ({ key, value })),
    );
    setEditError(null);
    setMode("edit");
  }

  const isDirty =
    mode === "edit" &&
    agent !== undefined &&
    (() => {
      if (draftModel.trim() !== (agent.model ?? "")) return true;
      // Prompt
      if (draftPrompt.trim() !== (agent.systemPrompt ?? "").trim()) return true;
      // Env
      const newEnv: Record<string, string> = {};
      for (const { key, value } of draftEnv) {
        const k = key.trim();
        if (k) newEnv[k] = value;
      }
      const oldEnv = agent.env ?? {};
      if (Object.keys(newEnv).length !== Object.keys(oldEnv).length) return true;
      if (Object.entries(newEnv).some(([k, v]) => oldEnv[k] !== v)) return true;
      return false;
    })();

  useEffect(() => {
    if (!isDirty) return;
    const handler = (e: BeforeUnloadEvent) => {
      e.preventDefault();
      e.returnValue = "";
    };
    window.addEventListener("beforeunload", handler);
    return () => window.removeEventListener("beforeunload", handler);
  }, [isDirty]);

  function cancelEdit() {
    if (isDirty && !window.confirm("Discard unsaved changes?")) {
      return;
    }
    setMode("view");
    setEditError(null);
  }

  async function handleSave() {
    if (!activeSlug || !agent) return;

    // Build patch with only changed fields so backend merge is minimal.
    const patch: {
      system_prompt?: string | null;
      model?: string | null;
      env?: Record<string, string>;
    } = {};

    const providerInfo = agent.provider ? PROVIDERS[agent.provider] : null;
    const modelEditable =
      agent.status === "offline" &&
      providerInfo !== null &&
      !providerInfo.modelOptional &&
      providerInfo.models.length > 0;
    const newModel = draftModel.trim();
    const oldModel = agent.model ?? "";
    const modelChanged = modelEditable && newModel !== oldModel;
    if (modelChanged) {
      patch.model = newModel === "" ? null : newModel;
    }

    const newPrompt = draftPrompt.trim();
    const oldPrompt = (agent.systemPrompt ?? "").trim();
    const promptChanged = newPrompt !== oldPrompt;
    if (promptChanged) {
      patch.system_prompt = newPrompt === "" ? null : newPrompt;
    }

    const newEnv: Record<string, string> = {};
    for (const { key, value } of draftEnv) {
      const k = key.trim();
      if (k) newEnv[k] = value;
    }
    const oldEnv = agent.env ?? {};
    const envChanged =
      Object.keys(newEnv).length !== Object.keys(oldEnv).length ||
      Object.entries(newEnv).some(([k, v]) => oldEnv[k] !== v);
    if (envChanged) patch.env = newEnv;

    if (Object.keys(patch).length === 0) {
      setMode("view");
      return;
    }

    setMode("saving");
    setEditError(null);
    const res = await client.updateAgent(activeSlug, agent.id, patch);
    if (res.ok && res.data?.agent) {
      updateAgent(agent.id, res.data.agent as Partial<Agent>);
      if (modelChanged && (res.data.agent.model ?? "") !== newModel) {
        setEditError(
          "Runtime did not apply the model change. Restart or update the runtime, then try again.",
        );
        setMode("edit");
        toast.error("Model was not updated");
        return;
      }
      setMode("view");

      // Generation-aware toast lines.
      const lines: string[] = [];
      if (envChanged) {
        lines.push("Environment → takes effect on next message");
      }
      if (modelChanged) {
        lines.push("Model → starts a fresh provider session on next start");
      }
      if (promptChanged) {
        lines.push(
          "System prompt → takes effect on next session (auto-rolls when current session fills)",
        );
      }
      toast.success("Saved", { description: lines.join("\n") });
    } else {
      setEditError(res.error ?? "Save failed");
      setMode("edit");
    }
  }

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
  const providerInfo = agent.provider ? PROVIDERS[agent.provider] : null;
  const modelOptions = providerInfo?.models ?? [];
  const showModelEditor =
    mode !== "view" &&
    providerInfo !== null &&
    !providerInfo.modelOptional &&
    modelOptions.length > 0;

  async function handleToggle() {
    if (!activeSlug) return;
    if (isRunning) {
      const res = await client.stopAgent(activeSlug, agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to stop agent");
      }
    } else {
      const res = await client.startAgent(activeSlug, agent!.id);
      if (res.ok && res.data?.agent) {
        updateAgent(agent!.id, res.data.agent as Partial<Agent>);
      } else if (!res.ok) {
        toast.error(res.error ?? "Failed to start agent");
      }
    }
  }

  return (
    <div className="h-full overflow-y-auto">
      <div className="p-6 max-w-3xl">
      <Button
        variant="ghost"
        size="sm"
        className="mb-4 text-text-secondary hover:text-foreground"
        onClick={() => {
          if (isDirty && !window.confirm("Discard unsaved changes?")) {
            return;
          }
          navigate("/management");
        }}
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
        {mode === "view" && (
          <Button
            variant="outline"
            size="sm"
            onClick={enterEditMode}
            className="border-border-strong hover:bg-surface-hover"
          >
            <Pencil className="size-4 mr-1.5" />
            Edit
          </Button>
        )}
      </div>

      {/* Info grid */}
      <div className="grid grid-cols-1 md:grid-cols-2 gap-5 mb-8 p-5 rounded-xl border border-border bg-card/50">
        <Field label="Repo Path">
          <code className="text-sm font-mono text-text-secondary bg-background/60 px-2 py-1 rounded">
            {agent.repoPath}
          </code>
        </Field>

        <Field label="Provider">
          <ProviderBadge provider={agent.provider} />
        </Field>

        <Field label="Model">
          {showModelEditor ? (
            <div className="space-y-1.5">
              <select
                value={draftModel}
                onChange={(e) => setDraftModel(e.target.value)}
                disabled={isRunning || mode === "saving"}
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                <option value="">— Provider default —</option>
                {modelOptions.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.label}
                  </option>
                ))}
              </select>
              {isRunning && (
                <p className="text-xs text-text-muted">
                  Stop the agent before changing model.
                </p>
              )}
            </div>
          ) : agent.model ? (
            <span className="inline-flex items-center px-2 py-0.5 rounded bg-background/60 border border-border text-sm font-mono">
              {agent.model}
            </span>
          ) : agent.provider === "opencode" ? (
            <span className="text-text-muted italic text-sm">
              Default (from opencode auth login)
            </span>
          ) : agent.provider === "pi" ? (
            <span className="text-text-muted italic text-sm">
              Default (from pi config)
            </span>
          ) : agent.provider === "hermes" ? (
            <span className="text-text-muted italic text-sm">
              Default (from hermes config)
            </span>
          ) : (
            <span className="text-text-muted">—</span>
          )}
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
          {mode === "view" ? (
            <div className="mt-2 rounded-xl border border-border bg-card/50 p-4">
              <pre className="text-sm whitespace-pre-wrap font-mono break-words text-text-secondary leading-relaxed">
                {agent.systemPrompt || "(none)"}
              </pre>
            </div>
          ) : (
            <Textarea
              value={draftPrompt}
              onChange={(e) => setDraftPrompt(e.target.value)}
              rows={4}
              className="mt-2 font-mono text-sm"
              placeholder="Describe the agent's role and behavior…"
            />
          )}
        </Field>
      </div>

      {/* Environment Variables */}
      <div className="mb-8">
        <Field label="Environment Variables">
          <p className="text-xs text-text-muted mt-1 mb-2">
            Injected as process env vars to the agent CLI. Flat key-value.
          </p>
          {mode === "view" ? (
            agent.env && Object.keys(agent.env).length > 0 ? (
              <div className="mt-2 rounded-xl border border-border bg-card/50 p-4 space-y-2">
                {Object.entries(agent.env).map(([key, value]) => (
                  <div key={key} className="text-sm font-mono flex items-center gap-2">
                    <span className="text-primary font-medium">{key}</span>
                    <span className="text-text-muted">=</span>
                    <span className="text-text-secondary">{value}</span>
                  </div>
                ))}
              </div>
            ) : (
              <p className="text-sm text-text-muted mt-2">(none)</p>
            )
          ) : (
            <EnvVarsEditor value={draftEnv} onChange={setDraftEnv} />
          )}
        </Field>
      </div>

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
        {mode === "view" ? (
          <>
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
          </>
        ) : (
          <>
            <Button
              variant="default"
              size="default"
              onClick={handleSave}
              disabled={mode === "saving"}
            >
              {mode === "saving" ? "Saving…" : "Save"}
            </Button>
            <Button
              variant="outline"
              size="default"
              onClick={cancelEdit}
              disabled={mode === "saving"}
            >
              Cancel
            </Button>
          </>
        )}
      </div>

      {editError && (
        <div className="mt-3 p-3 rounded-lg border border-destructive/30 bg-destructive/10 text-sm text-destructive">
          {editError}
        </div>
      )}

      <RemoveAgentDialog
        agentId={agent.id}
        agentName={agent.name}
        open={removeOpen}
        onOpenChange={setRemoveOpen}
        onRemoved={() => navigate("/management")}
      />
      </div>
    </div>
  );
}
