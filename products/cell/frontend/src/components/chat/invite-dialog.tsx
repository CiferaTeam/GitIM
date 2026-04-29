import { useState } from "react";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import * as client from "../../lib/client";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "../ui/dialog";
import { Button } from "../ui/button";
import { MemberPicker } from "./member-picker";

interface InviteDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  channel: string;
  allUsers: string[];
  excludeHandlers: string[];
  onInvited?: () => void | Promise<void>;
}

export function InviteDialog({
  open,
  onOpenChange,
  channel,
  allUsers,
  excludeHandlers,
  onInvited,
}: InviteDialogProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [selected, setSelected] = useState<string[]>([]);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  function resetLocalState() {
    setSelected([]);
    setSubmitting(false);
    setError(null);
  }

  function handleOpenChange(nextOpen: boolean) {
    if (!nextOpen) {
      resetLocalState();
    }
    onOpenChange(nextOpen);
  }

  async function handleInvite() {
    if (!activeSlug) {
      setError("No workspace selected");
      return;
    }
    setSubmitting(true);
    setError(null);
    try {
      const res = await client.joinChannel(activeSlug, channel, selected);
      if (!res.ok) {
        setError(res.error ?? "Failed to invite");
        setSubmitting(false);
        return;
      }
    } catch {
      setError("Network error — is the server running?");
      setSubmitting(false);
      return;
    }
    try {
      await onInvited?.();
    } catch { /* refresh failure is non-fatal — invite已成功 */ }
    resetLocalState();
    onOpenChange(false);
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>Invite members to #{channel}</DialogTitle>
        </DialogHeader>
        <div className="grid gap-3">
          <div className="grid gap-1.5">
            <label className="text-sm font-medium">Members</label>
            <MemberPicker
              allUsers={allUsers}
              excludeHandlers={excludeHandlers}
              value={selected}
              onChange={setSelected}
              placeholder="Search users to invite..."
            />
          </div>
          {error && (
            <p className="text-sm text-destructive">{error}</p>
          )}
          <DialogFooter>
            <Button
              variant="ghost"
              onClick={() => handleOpenChange(false)}
              disabled={submitting}
            >
              Cancel
            </Button>
            <Button
              onClick={handleInvite}
              disabled={selected.length === 0 || submitting}
            >
              {submitting ? "Inviting..." : "Invite"}
            </Button>
          </DialogFooter>
        </div>
      </DialogContent>
    </Dialog>
  );
}
