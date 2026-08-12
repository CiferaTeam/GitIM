import { Archive, ArchiveRestore, Copy, Loader2, SendHorizontal } from "lucide-react";
import { useState } from "react";

import { Button } from "@/components/ui/button";
import type { QuickSessionRuntimeOverlay } from "@/hooks/use-quick-session-store";
import { formatQuickSessionRef } from "@/lib/quick-session-ref";
import { formatTimestamp, type QuickSessionDetail } from "@/lib/types";
import { useTimezoneStore } from "@/hooks/use-timezone";

interface QuickSessionPanelProps {
  detail: QuickSessionDetail | null;
  runtime?: QuickSessionRuntimeOverlay;
  loading: boolean;
  error: string | null;
  sending: boolean;
  onSend: (body: string) => Promise<boolean>;
  onArchive: () => Promise<boolean>;
  onUnarchive: () => Promise<boolean>;
  onCopy: (ref: string) => void;
}

export function QuickSessionPanel({
  detail,
  runtime,
  loading,
  error,
  sending,
  onSend,
  onArchive,
  onUnarchive,
  onCopy,
}: QuickSessionPanelProps) {
  const [draft, setDraft] = useState("");
  const timezone = useTimezoneStore((state) => state.timezone);

  if (loading && !detail) {
    return (
      <div className="flex min-w-0 flex-1 items-center justify-center gap-2 text-xs text-text-muted">
        <Loader2 className="size-3.5 animate-spin" />
        Loading conversation…
      </div>
    );
  }
  if (error && !detail) {
    return <div className="flex min-w-0 flex-1 items-center justify-center px-4 text-xs text-destructive [overflow-wrap:anywhere]">{error}</div>;
  }
  if (!detail) {
    return (
      <div className="flex min-w-0 flex-1 items-center justify-center px-6 text-center text-xs leading-relaxed text-text-muted">
        Select a session or start a focused conversation.
      </div>
    );
  }

  const ref = formatQuickSessionRef(detail.meta.id);
  const activity = runtime?.latestEvent;
  return (
    <div className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex items-start justify-between gap-3 border-b border-border px-3 py-2.5">
        <div className="min-w-0">
          <h3 className="truncate text-sm font-semibold text-foreground">
            {detail.meta.title ?? "Untitled session"}
          </h3>
          <div className="mt-0.5 flex items-center gap-1.5 text-[11px] text-text-muted">
            <span>@{detail.meta.agent_id}</span>
            <span>·</span>
            <span>{runtime?.status ?? detail.meta.status}</span>
          </div>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button variant="ghost" size="icon-xs" onClick={() => onCopy(ref)} aria-label="Copy session reference">
            <Copy className="size-3.5" />
          </Button>
          {detail.archived ? (
            <Button variant="ghost" size="icon-xs" onClick={() => void onUnarchive()} aria-label="Unarchive session">
              <ArchiveRestore className="size-3.5" />
            </Button>
          ) : (
            <Button variant="ghost" size="icon-xs" onClick={() => void onArchive()} aria-label="Archive session">
              <Archive className="size-3.5" />
            </Button>
          )}
        </div>
      </div>

      {detail.meta.summary ? (
        <div className="border-b border-border bg-primary/5 px-3 py-2 text-xs leading-relaxed text-text-secondary [overflow-wrap:anywhere]">
          {detail.meta.summary}
        </div>
      ) : null}

      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto px-3 py-3">
        {detail.entries.map((entry) => (
          <div key={`${entry.line_number}-${entry.author}`} className="rounded-lg bg-surface/60 px-3 py-2">
            <div className="flex items-center gap-2 text-[11px] text-text-muted">
              <span className="font-medium text-foreground/80">@{entry.author}</span>
              <span className="font-mono">L{String(entry.line_number).padStart(6, "0")}</span>
              <span>{formatTimestamp(entry.timestamp, timezone)}</span>
            </div>
            <div className="mt-1 whitespace-pre-wrap text-xs leading-relaxed text-foreground/90 [overflow-wrap:anywhere]">
              {entry.body}
            </div>
          </div>
        ))}
        {activity ? (
          <div className="flex items-center gap-2 px-1 text-[11px] text-primary">
            {activity.event_type !== "done" && activity.event_type !== "error" ? (
              <Loader2 className="size-3 animate-spin" />
            ) : null}
            <span className="min-w-0 [overflow-wrap:anywhere]">{activity.detail}</span>
          </div>
        ) : null}
        {error ? (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-[11px] text-destructive [overflow-wrap:anywhere]">
            {error}
          </div>
        ) : null}
      </div>

      {!detail.archived ? (
        <form
          className="flex items-end gap-2 border-t border-border px-3 py-3"
          onSubmit={(event) => {
            event.preventDefault();
            const body = draft.trim();
            if (!body || sending) return;
            void onSend(body).then((ok) => {
              if (ok) setDraft("");
            });
          }}
        >
          <textarea
            rows={2}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            onKeyDown={(event) => {
              if (
                event.key !== "Enter" ||
                !event.metaKey ||
                event.nativeEvent.isComposing
              ) {
                return;
              }
              event.preventDefault();
              event.currentTarget.form?.requestSubmit();
            }}
            placeholder="Continue this session…"
            className="min-h-16 flex-1 resize-none rounded-lg border border-border bg-background px-3 py-2 text-xs text-foreground placeholder:text-text-muted focus:border-primary/60 focus:outline-none focus:ring-2 focus:ring-primary/20"
          />
          <Button type="submit" size="icon" disabled={sending || draft.trim().length === 0} aria-label="Send Quick Session message">
            {sending ? <Loader2 className="size-4 animate-spin" /> : <SendHorizontal className="size-4" />}
          </Button>
        </form>
      ) : null}
    </div>
  );
}
