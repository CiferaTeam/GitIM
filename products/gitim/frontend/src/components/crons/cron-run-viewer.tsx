import { useEffect, useMemo, useState } from "react";
import { ChevronLeft, Loader2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { MessageBody } from "@/components/chat/message-body";
import * as client from "@/lib/client";
import { parseThread, type ThreadEntry } from "@/daemon-web/parser";
import { formatTimestamp } from "@/lib/types";

interface CronRunViewerProps {
  slug: string | null;
  cronName: string;
  ts: string;
  onBack: () => void;
}

/**
 * Renders the body of one cron fire by parsing the on-disk `.thread` file
 * with the daemon-web parser (single source of truth for the line format)
 * and reusing `MessageBody` for each entry. Cron threads are typically
 * short — one [@system] prompt plus an optional agent reply — so a flat
 * vertical list is enough; we skip the chat's reply/threading affordances.
 */
export function CronRunViewer({ slug, cronName, ts, onBack }: CronRunViewerProps) {
  const [body, setBody] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!slug) return;
    let cancelled = false;
    setLoading(true);
    setError(null);
    setBody(null);
    (async () => {
      const res = await client.getCronRunBody(slug, cronName, ts);
      if (cancelled) return;
      if (!res.ok || !res.data) {
        setError(res.error ?? "Failed to load run body");
        setLoading(false);
        return;
      }
      setBody(res.data.body);
      setLoading(false);
    })().catch((err: unknown) => {
      if (cancelled) return;
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
    });
    return () => {
      cancelled = true;
    };
  }, [slug, cronName, ts]);

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
                  <span>{formatTimestamp(entry.timestamp)}</span>
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
