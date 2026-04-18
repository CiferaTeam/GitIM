import { useEffect, useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { LabelChipInput } from "@/components/ui/label-chip-input";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useCardStore, selectAllLabels } from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { Card, CardStatus } from "@/lib/types";

export interface CardCreateDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Pre-fill the channel select and disable it. Used from channel drawer. */
  presetChannel?: string;
}

const STATUSES: CardStatus[] = ["todo", "doing", "done"];

export function CardCreateDialog({
  open,
  onOpenChange,
  presetChannel,
}: CardCreateDialogProps) {
  const navigate = useNavigate();
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const channels = useChatStore((s) => s.channels);
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);
  const upsertCard = useCardStore((s) => s.upsertCard);
  const allLabels = useCardStore(selectAllLabels);
  const currentUser = useChatStore((s) => s.currentUser);

  const channelOptions = useMemo(
    () => channels.filter((c) => c.kind === "channel").map((c) => c.name),
    [channels],
  );

  const assigneeOptions = useMemo(() => {
    const set = new Set<string>([...users, ...agents.map((a) => a.id)]);
    return [...set].sort();
  }, [users, agents]);

  const [title, setTitle] = useState("");
  const [channel, setChannel] = useState(presetChannel ?? "");
  const [assignee, setAssignee] = useState<string>("");
  const [labels, setLabels] = useState<string[]>([]);
  const [status, setStatus] = useState<CardStatus>("todo");
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (open) {
      // Reset form each time dialog (re)opens so stale aborted-create state
      // doesn't leak into the next session.
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setTitle("");
      setChannel(presetChannel ?? "");
      setAssignee("");
      setLabels([]);
      setStatus("todo");
      setError(null);
      setSubmitting(false);
    }
  }, [open, presetChannel]);

  const canSubmit = title.trim().length > 0 && channel.length > 0 && !submitting;

  async function handleSubmit() {
    if (!canSubmit) return;
    if (!activeSlug) {
      setError("No workspace selected");
      return;
    }
    setSubmitting(true);
    setError(null);
    const res = await client.createCard(activeSlug, channel, title.trim(), {
      labels: labels.length > 0 ? labels : undefined,
      assignee: assignee || undefined,
      status,
    });
    if (!res.ok || !res.data) {
      setError(res.error ?? "Failed to create");
      setSubmitting(false);
      return;
    }
    const nowIso = new Date()
      .toISOString()
      .replace(/[-:]/g, "")
      .replace(/\.\d+/, "");
    const newCard: Card = {
      card_id: res.data.card_id,
      channel: res.data.channel,
      title: res.data.title,
      status,
      labels,
      assignee: assignee || null,
      created_by: currentUser,
      created_at: nowIso,
      updated_at: nowIso,
    };
    upsertCard(newCard);
    onOpenChange(false);
    toast.success("Card created");
    navigate(`/cards/${newCard.channel}/${newCard.card_id}`);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>New card</DialogTitle>
        </DialogHeader>

        <div className="flex flex-col gap-3">
          <Field label="Title">
            <Input
              value={title}
              onChange={(e) => setTitle(e.target.value)}
              placeholder="What needs doing?"
              maxLength={200}
              autoFocus
              disabled={submitting}
            />
          </Field>

          <Field label="Channel">
            <select
              value={channel}
              onChange={(e) => setChannel(e.target.value)}
              disabled={!!presetChannel || submitting}
              className="w-full rounded-md border border-border bg-muted/20 px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
            >
              <option value="">Select a channel…</option>
              {channelOptions.map((c) => (
                <option key={c} value={c}>
                  #{c}
                </option>
              ))}
            </select>
          </Field>

          <Field label="Assignee (optional)">
            <select
              value={assignee}
              onChange={(e) => setAssignee(e.target.value)}
              disabled={submitting}
              className="w-full rounded-md border border-border bg-muted/20 px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
            >
              <option value="">Unassigned</option>
              {assigneeOptions.map((a) => (
                <option key={a} value={a}>
                  @{a}
                </option>
              ))}
            </select>
          </Field>

          <Field label="Labels (optional)">
            <LabelChipInput
              value={labels}
              onChange={setLabels}
              suggestions={allLabels}
              allowCreate
              placeholder="Type label and press Enter"
            />
          </Field>

          <Field label="Status">
            <select
              value={status}
              onChange={(e) => setStatus(e.target.value as CardStatus)}
              disabled={submitting}
              className="w-full rounded-md border border-border bg-muted/20 px-2 py-1.5 text-sm focus:outline-none focus:ring-1 focus:ring-ring"
            >
              {STATUSES.map((s) => (
                <option key={s} value={s} className="capitalize">
                  {s}
                </option>
              ))}
            </select>
          </Field>

          {error && <p className="text-xs text-destructive">{error}</p>}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={submitting}
          >
            Cancel
          </Button>
          <Button onClick={handleSubmit} disabled={!canSubmit}>
            {submitting ? "Creating…" : "Create card"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}

function Field({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <label className="flex flex-col gap-1">
      <span className="text-xs font-medium text-muted-foreground">{label}</span>
      {children}
    </label>
  );
}
