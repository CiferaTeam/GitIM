import { Button } from "@/components/ui/button";
import { Card, CardContent, CardFooter, CardHeader } from "@/components/ui/card";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { ArchivedUserEntry } from "@/lib/client";
import { Archive, Loader2, RotateCcw } from "lucide-react";
import { useState } from "react";
import { toast } from "sonner";

interface ArchivedAgentCardProps {
  entry: ArchivedUserEntry;
  /** Called after a successful unarchive so the parent can drop this row. */
  onUnarchived?: (handler: string) => void;
}

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

/**
 * Read-only card for an agent whose user.meta.yaml lives under
 * `archive/users/`. Runtime metadata (provider / model / messages_processed
 * / status / activity) is gone after burn — the agent's clone is deleted —
 * so we render only what the daemon can supply: handler and (best-effort)
 * display_name. The single available action is "Unarchive User", which
 * runs the daemon-side reverse of `depart_user` and exposes the recovery
 * path on the show-archived view of the agent list.
 */
export function ArchivedAgentCard({
  entry,
  onUnarchived,
}: ArchivedAgentCardProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const [submitting, setSubmitting] = useState(false);

  const displayName = entry.display_name ?? entry.handler;

  async function handleUnarchive() {
    if (!activeSlug) return;
    setSubmitting(true);
    const res = await client.unarchiveUser(activeSlug, entry.handler);
    setSubmitting(false);
    if (res.ok) {
      toast.success(`@${entry.handler} unarchived`, {
        description:
          "User profile restored. Re-add the agent to spin up a runtime instance.",
      });
      onUnarchived?.(entry.handler);
    } else {
      toast.error(res.error ?? "Failed to unarchive user");
    }
  }

  return (
    <Card className="relative overflow-hidden bg-card/40 border-dashed">
      {/* Status bar — slate gray to read as inactive */}
      <div className="absolute top-0 left-0 right-0 h-1 bg-text-muted" />

      <CardHeader className="pb-2 pt-5">
        <div className="flex items-center justify-between gap-3">
          <div className="flex items-center gap-3 min-w-0">
            <div className="w-10 h-10 rounded-xl flex items-center justify-center text-sm font-bold bg-surface text-text-muted border border-border shrink-0">
              {initials(displayName)}
            </div>
            <div className="min-w-0">
              <span className="font-semibold text-lg truncate block text-text-secondary">
                {displayName}
              </span>
              <span className="text-xs text-text-muted truncate block font-mono">
                @{entry.handler}
              </span>
            </div>
          </div>
          <span className="inline-flex items-center gap-1 px-2 py-0.5 rounded-md bg-surface text-text-muted text-xs font-medium border border-border">
            <Archive className="size-3" />
            Archived
          </span>
        </div>
      </CardHeader>

      <CardContent>
        <p className="text-sm text-text-muted">
          Runtime metadata is unavailable — the agent's clone was deleted on
          burn. Unarchive restores the user profile in the shared repo so the
          handler appears again, but the runtime instance must be re-added to
          go live.
        </p>
      </CardContent>

      <CardFooter className="gap-2 flex-wrap">
        <Button
          variant="outline"
          size="sm"
          onClick={handleUnarchive}
          disabled={submitting}
          className="border-border-strong hover:bg-surface-hover"
        >
          {submitting ? (
            <Loader2 className="size-3.5 mr-1 animate-spin" />
          ) : (
            <RotateCcw className="size-3.5 mr-1" />
          )}
          Unarchive User
        </Button>
      </CardFooter>
    </Card>
  );
}
