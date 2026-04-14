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
import * as client from "@/lib/client";
import { toHandler, validateHandler } from "@/lib/client";
import type { Agent } from "@/lib/types";
import { Plus } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

export function AddAgentDialog() {
  const addAgent = useAgentStore((s) => s.addAgent);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [model, setModel] = useState("claude-sonnet-4-6");
  const [systemPrompt, setSystemPrompt] = useState("");
  const [envVars, setEnvVars] = useState<{ key: string; value: string }[]>([]);
  const [submitting, setSubmitting] = useState(false);

  const handler = toHandler(name.trim());
  const validationError = name.trim() ? validateHandler(name.trim()) : null;

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim() || validationError || submitting) return;

    const envMap: Record<string, string> = {};
    for (const { key, value } of envVars) {
      if (key.trim()) envMap[key.trim()] = value;
    }

    setSubmitting(true);
    try {
      const res = await client.addAgent(
        name.trim(),
        systemPrompt.trim(),
        model,
        envMap,
      );
      if (res.ok && res.data?.agent) {
        addAgent(res.data.agent as Agent);
        setName("");
        setModel("claude-sonnet-4-6");
        setSystemPrompt("");
        setEnvVars([]);
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

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>Add Agent</DialogTitle>
          </DialogHeader>

          <form onSubmit={handleSubmit} className="space-y-4">
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
                className="flex h-9 w-full rounded-md border border-input bg-transparent px-3 py-1 text-sm shadow-sm transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <option value="claude-sonnet-4-6">Claude Sonnet 4.6</option>
                <option value="claude-opus-4-6">Claude Opus 4.6</option>
                <option value="claude-haiku-4-5">Claude Haiku 4.5</option>
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
                onClick={() => setOpen(false)}
              >
                Cancel
              </Button>
              <Button type="submit" disabled={!name.trim() || !!validationError || submitting}>
                Add
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </>
  );
}
