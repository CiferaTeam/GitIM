import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MessageBody } from "@/components/chat/message-body";
import { useTimezoneStore } from "@/hooks/use-timezone";
import * as client from "@/lib/client";
import { parseThread, type ThreadEntry } from "@/daemon-web/parser";
// `entry.timestamp` from `parseThread` is the LEGACY compact format
// (`YYYYMMDDTHHMMSSZ`) that gitim writes on the message line, NOT the
// RFC 3339 `entry.ts` that the calendar timeline uses elsewhere on this
// page. `formatTimestamp` handles the legacy form. Two timestamp formats
// coexist in the cron UI on purpose: thread bodies inherit gitim's line
// format, calendar metadata uses RFC 3339 for portability.
import { formatTimestamp } from "@/lib/types";

interface CronRunViewerProps {
  slug: string | null;
  cronName: string;
  ts: string;
  onBack: () => void;
}

interface RunBodyState {
  key: string | null;
  body: string | null;
  error: string | null;
}

/**
 * Renders the body of one cron fire by parsing the on-disk `.thread` file
 * with the daemon-web parser (single source of truth for the line format)
 * and reusing `MessageBody` for each entry. Cron threads are typically
 * short — one [@system] prompt plus an optional agent reply — so a flat
 * vertical list is enough; we skip the chat's reply/threading affordances.
 */
export function CronRunViewer({ slug, cronName, ts, onBack }: CronRunViewerProps) {
  const timezone = useTimezoneStore((s) => s.timezone);
  const requestKey = slug ? `${slug}\0${cronName}\0${ts}` : null;
  const [runState, setRunState] = useState<RunBodyState>({
    key: null,
    body: null,
    error: null,
  });
  const isCurrentRequest = runState.key === requestKey;
  const body = isCurrentRequest ? runState.body : null;
  const error = isCurrentRequest ? runState.error : null;
  const loading = requestKey !== null && !isCurrentRequest;

  useEffect(() => {
    if (!slug || requestKey === null) return;
    // AbortController so rapidly switching between cron runs cancels the
    // previous in-flight fetch (matches the `useCronTimeline` pattern).
    // Otherwise tapping through five runs on mobile burns bandwidth on
    // four bodies the user no longer cares about, and the resolutions
    // race the latest one to set state.
    const controller = new AbortController();
    (async () => {
      const res = await client.getCronRunBody(slug, cronName, ts, controller.signal);
      if (controller.signal.aborted) return;
      if (!res.ok || !res.data) {
        setRunState({
          key: requestKey,
          body: null,
          error: res.error ?? "Failed to load run body",
        });
        return;
      }
      setRunState({ key: requestKey, body: res.data.body, error: null });
    })().catch((err: unknown) => {
      if (controller.signal.aborted) return;
      // Aborts surface as DOMException("AbortError") on real fetch and as
      // plain Error("AbortError") in some jsdom paths — drop both.
      if (err instanceof DOMException && err.name === "AbortError") return;
      if (err instanceof Error && err.name === "AbortError") return;
      setRunState({
        key: requestKey,
        body: null,
        error: err instanceof Error ? err.message : String(err),
      });
    });
    return () => {
      controller.abort();
    };
  }, [slug, cronName, ts, requestKey]);

  const entries = useMemo<ThreadEntry[]>(() => {
    if (!body) return [];
    try {
      return parseThread(body).entries;
    } catch {
      // parseThread is defensive — but if it ever throws on a malformed
      // body, fall back to rendering the raw text so the user can still
      // see what's there.
      return [];
    }
  }, [body]);

  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center gap-2 border-b border-border px-4 py-3">
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onBack}
          aria-label="Back to day"
        >
          <ChevronLeft className="size-4" />
        </Button>
        <div className="min-w-0">
          <h2 className="truncate text-sm font-semibold font-mono">
            cron({cronName})
          </h2>
          <p className="truncate text-xs text-muted-foreground font-mono">{ts}</p>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto px-4 py-3">
        {loading && (
          <div
            className="flex items-center gap-2 text-sm text-muted-foreground"
            role="status"
          >
            <Loader2 className="size-4 animate-spin" /> Loading run...
          </div>
        )}
        {error && (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}
        {!loading && !error && entries.length === 0 && body && (
          <pre className="whitespace-pre-wrap break-words font-mono text-xs text-text-secondary">
            {body}
          </pre>
        )}
        {entries.length > 0 && (
          <ol className="space-y-3">
            {entries.map((entry) => (
              <li
                key={entry.line_number}
                className="rounded-md border border-border bg-surface/40 px-3 py-2"
              >
                <div className="mb-1 flex items-center gap-2 text-[11px] text-muted-foreground font-mono">
                  <span className="text-text-secondary">@{entry.author}</span>
                  <span>·</span>
                  <span>{formatTimestamp(entry.timestamp, timezone)}</span>
                  <span className="ml-auto opacity-60">
                    L{String(entry.line_number).padStart(6, "0")}
                  </span>
                </div>
                {entry.type === "event" ? (
                  <span className="text-xs italic text-text-muted">
                    event: {entry.event_type}
                  </span>
                ) : (
                  <div className="text-sm leading-6 text-foreground/90">
                    <MessageBody body={entry.body} />
                  </div>
                )}
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}
