import { useEffect, useState } from "react";
import { AlertTriangle, ChevronLeft, Loader2 } from "lucide-react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { useTimezoneStore } from "@/hooks/use-timezone";
import * as client from "@/lib/client";
import { formatDateTime } from "@/lib/timezone";
import type { CronDetail } from "@/lib/types";

interface CronSpecDetailProps {
  slug: string | null;
  cronName: string;
  /** Time of the entry that opened this view. For `kind === "missed"`,
   *  rendered as a badge at the top. `kind === "future"` shows just the
   *  spec body without the badge. */
  missedTs?: string;
  futureTs?: string;
  onBack: () => void;
}

/** Reads `spec` (the raw yaml-as-json from the daemon) defensively — the
 *  daemon's CronSpec validator guarantees these fields exist on create,
 *  but a hand-edited spec.yaml could regress, and rendering "undefined"
 *  is worse than a fallback string. */
function asString(value: unknown, fallback = "—"): string {
  if (typeof value === "string" && value.length > 0) return value;
  if (typeof value === "number" || typeof value === "boolean") return String(value);
  return fallback;
}

interface CronDetailState {
  key: string | null;
  detail: CronDetail | null;
  error: string | null;
}

export function CronSpecDetail({
  slug,
  cronName,
  missedTs,
  futureTs,
  onBack,
}: CronSpecDetailProps) {
  const displayTimezone = useTimezoneStore((s) => s.timezone);
  const requestKey = slug ? `${slug}\0${cronName}` : null;
  const [detailState, setDetailState] = useState<CronDetailState>({
    key: null,
    detail: null,
    error: null,
  });
  const isCurrentRequest = detailState.key === requestKey;
  const detail = isCurrentRequest ? detailState.detail : null;
  const error = isCurrentRequest ? detailState.error : null;
  const loading = requestKey !== null && !isCurrentRequest;

  useEffect(() => {
    if (!slug || requestKey === null) return;
    // AbortController per the `useCronTimeline` pattern — see
    // `cron-run-viewer.tsx` for the rationale.
    const controller = new AbortController();
    (async () => {
      const res = await client.showCron(slug, cronName, controller.signal);
      if (controller.signal.aborted) return;
      if (!res.ok || !res.data) {
        setDetailState({
          key: requestKey,
          detail: null,
          error: res.error ?? "Failed to load cron spec",
        });
        return;
      }
      setDetailState({ key: requestKey, detail: res.data, error: null });
    })().catch((err: unknown) => {
      if (controller.signal.aborted) return;
      if (err instanceof DOMException && err.name === "AbortError") return;
      if (err instanceof Error && err.name === "AbortError") return;
      setDetailState({
        key: requestKey,
        detail: null,
        error: err instanceof Error ? err.message : String(err),
      });
    });
    return () => {
      controller.abort();
    };
  }, [slug, cronName, requestKey]);

  const spec = detail?.spec ?? {};
  const schedule = asString(spec["schedule"]);
  const timezone = asString(spec["timezone"], "UTC");
  const target = asString(spec["target"]);
  const prompt = asString(spec["prompt"], "");
  const createdBy = asString(spec["created_by"]);
  const createdAt = asString(spec["created_at"]);
  const enabled = spec["enabled"] !== false;

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
        <div className="min-w-0 flex-1">
          <h2 className="truncate text-sm font-semibold font-mono">{cronName}</h2>
          <p className="truncate text-xs text-muted-foreground">
            {enabled ? "已启用" : "已暂停"}
          </p>
        </div>
      </header>

      <div className="flex-1 overflow-y-auto px-4 py-3 space-y-4">
        {missedTs && (
          <div
            role="status"
            className="flex items-start gap-2 rounded-md border border-error/30 bg-error/10 px-3 py-2 text-xs text-error"
          >
            <AlertTriangle className="size-4 shrink-0" />
            <div>
              <p className="font-medium">
                missed at {formatDateTime(missedTs, displayTimezone)}
              </p>
              <p className="mt-0.5 text-error/80">runtime 当时未运行，到点没能发出消息</p>
            </div>
          </div>
        )}
        {futureTs && (
          <div className="rounded-md border border-primary/30 bg-primary/10 px-3 py-2 text-xs text-primary">
            预计 fire 时刻：
            <span className="font-mono">
              {formatDateTime(futureTs, displayTimezone)}
            </span>
          </div>
        )}

        {loading && (
          <div
            className="flex items-center gap-2 text-sm text-muted-foreground"
            role="status"
          >
            <Loader2 className="size-4 animate-spin" /> Loading spec...
          </div>
        )}
        {error && (
          <div className="rounded-md border border-destructive/30 bg-destructive/10 px-3 py-2 text-sm text-destructive">
            {error}
          </div>
        )}
        {detail && (
          <div className="space-y-3 text-sm">
            <SpecField label="Schedule">
              <code className="font-mono text-xs bg-muted px-1.5 py-0.5 rounded">
                {schedule}
              </code>
            </SpecField>
            <SpecField label="Timezone">
              <span className="font-mono text-xs">{timezone}</span>
            </SpecField>
            <SpecField label="Target">
              <span className="font-mono text-xs text-primary">@{target}</span>
            </SpecField>
            <SpecField label="Status">
              <Badge variant={enabled ? "default" : "secondary"}>
                {enabled ? "enabled" : "disabled"}
              </Badge>
            </SpecField>
            <SpecField label="Prompt">
              <pre className="whitespace-pre-wrap break-words rounded-md border border-border bg-surface/40 px-3 py-2 font-sans text-xs leading-5 text-foreground/90">
                {prompt}
              </pre>
            </SpecField>
            <SpecField label="Created">
              <span className="text-xs">
                <span className="text-text-secondary font-mono">@{createdBy}</span>
                <span className="text-text-muted"> · </span>
                <span className="font-mono">
                  {formatDateTime(createdAt, displayTimezone)}
                </span>
              </span>
            </SpecField>
            {detail.next_fire && (
              <SpecField label="Next fire">
                <span className="font-mono text-xs text-primary">
                  {formatDateTime(detail.next_fire, displayTimezone)}
                </span>
              </SpecField>
            )}
            {detail.recent_runs.length > 0 && (
              <SpecField label="Recent runs">
                <ul className="space-y-0.5 font-mono text-[11px] text-text-secondary">
                  {detail.recent_runs.slice(0, 5).map((run) => (
                    <li key={run.ts}>
                      {formatDateTime(run.ts, displayTimezone)}
                    </li>
                  ))}
                </ul>
              </SpecField>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

function SpecField({
  label,
  children,
}: {
  label: string;
  children: React.ReactNode;
}) {
  return (
    <div>
      <p className="mb-1 text-[10px] uppercase tracking-wide text-text-muted">
        {label}
      </p>
      {children}
    </div>
  );
}
