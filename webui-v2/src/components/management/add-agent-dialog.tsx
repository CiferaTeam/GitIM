import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { toHandler, validateHandler } from "@/lib/client";
import {
  PROVIDER_IDS,
  PROVIDERS,
  type PreflightResult,
  type ProviderId,
} from "@/lib/providers";
import type { Agent } from "@/lib/types";
import { CheckCircle2, Loader2, Plus, XCircle } from "lucide-react";
import { useRef, useState } from "react";
import { toast } from "sonner";

export function AddAgentDialog() {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const addAgent = useAgentStore((s) => s.addAgent);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [provider, setProvider] = useState<ProviderId | "">("");
  const [model, setModel] = useState("");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [detecting, setDetecting] = useState(false);
  const [detectResult, setDetectResult] = useState<PreflightResult | null>(null);
  // Generation counter guards against stale preflight responses when the user
  // switches provider mid-flight or fires Detect multiple times in succession.
  const detectSeq = useRef(0);

  const handler = toHandler(name.trim());
  const validationError = name.trim() ? validateHandler(name.trim()) : null;
  const availableModels = provider ? PROVIDERS[provider].models : [];

  function resetForm() {
    setName("");
    setProvider("");
    setModel("");
    setSystemPrompt("");
    setEnvVars([]);
    setSubmitting(false);
    setDetecting(false);
    setDetectResult(null);
    detectSeq.current += 1;
  }

  async function handleDetect() {
    if (!provider || detecting) return;
    const seq = ++detectSeq.current;
    setDetecting(true);
    setDetectResult(null);
    const res = await client.preflightProvider(provider as ProviderId);
    // Bail out if the user switched provider (or fired another detect) while
    // the request was in flight — a stale response must not overwrite state.
    if (seq !== detectSeq.current) return;
    if (res.ok && res.data) {
      setDetectResult(res.data);
    } else {
      setDetectResult({
        available: false,
        provider: provider as string,
        version: null,
        model_used: null,
        duration_ms: 0,
        output_preview: null,
        error: res.error ?? "Request failed",
        error_kind: "other",
      });
    }
    setDetecting(false);
  }

  function detectErrorMessage(result: PreflightResult): string {
    switch (result.error_kind) {
      case "not_installed":
        return "CLI not found. Install claude/codex and retry.";
      case "timeout":
        return "Timed out.";
      default:
        return result.error ?? "Unknown error";
    }
  }

  function handleOpenChange(next: boolean) {
    setOpen(next);
    if (!next) resetForm();
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (
      !name.trim() ||
      validationError ||
      submitting ||
      !provider ||
      !model ||
      !detectResult?.available
    )
      return;
    if (!activeSlug) {
      toast.error("No workspace selected");
      return;
    }

    const envMap: Record<string, string> = {};
    for (const { key, value } of envVars) {
      if (key.trim()) envMap[key.trim()] = value;
    }

    setSubmitting(true);
    try {
      const res = await client.addAgent(
        activeSlug,
        name.trim(),
        provider,
        systemPrompt.trim(),
        model,
        envMap,
      );
      if (res.ok && res.data?.agent) {
        addAgent(res.data.agent as Agent);
        resetForm();
        setOpen(false);
      } else {
        toast.error(res.error ?? "Failed to add agent");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <>
      <Button onClick={() => setOpen(true)}>
        <Plus className="size-4 mr-1" />
        Add Agent
      </Button>

      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add Agent</DialogTitle>
          </DialogHeader>

          <form onSubmit={handleSubmit} className="space-y-4">
            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-provider">
                Provider
              </label>
              <select
                id="agent-provider"
                value={provider}
                onChange={(e) => {
                  setProvider(e.target.value as ProviderId | "");
                  setModel("");
                  // Invalidate any in-flight detect so its late-arriving
                  // response can't clobber the cleared state.
                  detectSeq.current += 1;
                  setDetectResult(null);
                }}
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <option value="">— Select provider —</option>
                {PROVIDER_IDS.map((id) => (
                  <option key={id} value={id}>
                    {PROVIDERS[id].label}
                  </option>
                ))}
              </select>
              <div className="flex items-center gap-2 pt-1">
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={handleDetect}
                  disabled={!provider || detecting}
                >
                  {detecting ? (
                    <>
                      <Loader2 className="size-4 mr-1 animate-spin" />
                      Detecting...
                    </>
                  ) : (
                    "Detect"
                  )}
                </Button>
                {detecting && (
                  <span className="text-sm text-muted-foreground">
                    Detecting...
                  </span>
                )}
                {!detecting && detectResult?.available === true && (
                  <span className="flex items-center gap-1 text-sm text-green-600">
                    <CheckCircle2 className="size-4" />
                    OK — {detectResult.duration_ms} ms
                  </span>
                )}
                {!detecting && detectResult?.available === false && (
                  <span className="flex items-center gap-1 text-sm text-red-600">
                    <XCircle className="size-4" />
                    {detectErrorMessage(detectResult)}
                  </span>
                )}
              </div>
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-name">
                Name
              </label>
              <Input
                id="agent-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder="e.g. Code Reviewer"
                required
              />
              {handler && !validationError && (
                <p className="text-xs text-muted-foreground">
                  Handler: <code>{handler}</code>
                </p>
              )}
              {validationError && (
                <p className="text-xs text-destructive">{validationError}</p>
              )}
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-model">
                Model
              </label>
              <select
                id="agent-model"
                value={model}
                onChange={(e) => setModel(e.target.value)}
                disabled={!provider}
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50"
              >
                <option value="">— Select model —</option>
                {availableModels.map((m) => (
                  <option key={m.id} value={m.id}>
                    {m.label}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium" htmlFor="agent-prompt">
                System Prompt
              </label>
              <Textarea
                id="agent-prompt"
                rows={4}
                value={systemPrompt}
                onChange={(e) => setSystemPrompt(e.target.value)}
                placeholder="Describe the agent's role and behavior…"
              />
            </div>

            <div className="space-y-1.5">
              <label className="text-sm font-medium">
                Environment Variables
              </label>
              <div className="space-y-2">
                {envVars.map((pair, i) => (
                  <div key={i} className="flex gap-2">
                    <Input
                      placeholder="KEY"
                      value={pair.key}
                      onChange={(e) => {
                        const updated = [...envVars];
                        updated[i] = { ...updated[i], key: e.target.value };
                        setEnvVars(updated);
                      }}
                      className="flex-1 font-mono text-xs"
                    />
                    <Input
                      placeholder="value"
                      value={pair.value}
                      onChange={(e) => {
                        const updated = [...envVars];
                        updated[i] = { ...updated[i], value: e.target.value };
                        setEnvVars(updated);
                      }}
                      className="flex-1 font-mono text-xs"
                    />
                    <Button
                      type="button"
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        setEnvVars(envVars.filter((_, j) => j !== i))
                      }
                      className="px-2 text-muted-foreground hover:text-destructive"
                    >
                      ×
                    </Button>
                  </div>
                ))}
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() =>
                    setEnvVars([...envVars, { key: "", value: "" }])
                  }
                >
                  + Add Variable
                </Button>
              </div>
            </div>

            <DialogFooter>
              <Button
                type="button"
                variant="outline"
                onClick={() => handleOpenChange(false)}
              >
                Cancel
              </Button>
              <Button
                type="submit"
                disabled={
                  !name.trim() ||
                  !!validationError ||
                  submitting ||
                  !provider ||
                  !model ||
                  !detectResult?.available
                }
              >
                Add
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
