import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { cn } from "@/lib/utils";
import { Loader2, Trash2 } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

interface RemoveAgentDialogProps {
  agentId: string;
  agentName: string;
  agentPath?: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called after the agent has been removed from the store. */
  onRemoved?: () => void;
}

type RemoveMode = "soft" | "hard";

export function RemoveAgentDialog({
  agentId,
  agentName,
  agentPath,
  open,
  onOpenChange,
  onRemoved,
}: RemoveAgentDialogProps) {
  const removeAgent = useAgentStore((s) => s.removeAgent);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [mode, setMode] = useState<RemoveMode>("soft");
  const [submitting, setSubmitting] = useState(false);

  function closeDialog() {
    setMode("soft");
    setSubmitting(false);
    onOpenChange(false);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (nextOpen) {
      onOpenChange(true);
    } else {
      closeDialog();
    }
  }

  async function handleConfirm() {
    if (!activeSlug) {
      toast.error("No workspace selected");
      return;
    }
    setSubmitting(true);
    const res = await client.removeAgent(activeSlug, agentId, {
      hardDelete: mode === "hard",
    });
    setSubmitting(false);
    if (res.ok) {
      removeAgent(agentId);
      closeDialog();
      onRemoved?.();
    } else {
      toast.error(res.error ?? "Failed to remove agent");
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Remove agent?</DialogTitle>
          <DialogDescription>
            Choose how to remove{" "}
            <span className="font-medium text-foreground">{agentName}</span>.
          </DialogDescription>
        </DialogHeader>
        <div className="grid gap-2">
          <RemoveModeOption
            value="soft"
            checked={mode === "soft"}
            onChange={setMode}
            title="Soft delete"
            description="Stop the agent and remove it from the runtime. The local directory stays on disk."
          />
          <RemoveModeOption
            value="hard"
            checked={mode === "hard"}
            onChange={setMode}
            title="Hard delete"
            description="Stop the agent and delete its local directory from this workspace."
          />
        </div>
        {mode === "hard" && agentPath && (
          <p className="rounded-md border border-destructive/30 bg-destructive/10 p-2 font-mono text-[11px] break-all text-destructive">
            {agentPath}
          </p>
        )}
        <DialogFooter>
          <Button
            variant="outline"
            onClick={closeDialog}
            disabled={submitting}
          >
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm} disabled={submitting}>
            {submitting ? (
              <Loader2 className="size-4 animate-spin" />
            ) : (
              <Trash2 className="size-4" />
            )}
            {mode === "hard" ? "Hard delete" : "Soft delete"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function RemoveModeOption({
  value,
  checked,
  onChange,
  title,
  description,
}: {
  value: RemoveMode;
  checked: boolean;
  onChange: (value: RemoveMode) => void;
  title: string;
  description: string;
}) {
  return (
    <label
      className={cn(
        "flex cursor-pointer gap-3 rounded-md border p-3 transition-colors",
        checked
          ? "border-primary/60 bg-primary/10"
          : "border-border bg-surface/40 hover:bg-surface-hover",
      )}
    >
      <input
        type="radio"
        name="remove-agent-mode"
        value={value}
        checked={checked}
        onChange={() => onChange(value)}
        className="mt-0.5 size-4 accent-primary"
      />
      <span className="space-y-1">
        <span className="block text-sm font-medium text-foreground">{title}</span>
        <span className="block text-xs leading-5 text-muted-foreground">
          {description}
        </span>
      </span>
    </label>
  );
}
