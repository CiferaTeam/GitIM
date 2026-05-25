import { useState } from "react";
import { RefreshCcw, RotateCcw, Unplug } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useChatStore } from "@/hooks/use-chat-store";
import { useConnectionDiagnosticsStore } from "@/hooks/use-connection-diagnostics-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import * as client from "@/lib/client";

function formatTime(value: string | null): string {
  if (!value) return "Never";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;
  return new Intl.DateTimeFormat(undefined, {
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  }).format(date);
}

function shortCommit(value: string | null): string {
  return value ? value.slice(0, 8) : "Unknown";
}

function modeLabel(mode: "local" | "remote"): string {
  return mode === "local" ? "Browser" : "Runtime";
}

function browserSyncLabel(status: string): string {
  switch (status) {
    case "idle":
      return "Idle";
    case "syncing":
      return "Syncing";
    case "error":
      return "Error";
    case "reconnect_required":
      return "Reconnect required";
    default:
      return "Unknown";
  }
}

function DiagnosticsRow({
  label,
  value,
}: {
  label: string;
  value: string;
}) {
  return (
    <div className="grid grid-cols-[88px_1fr] gap-2 text-xs">
      <span className="text-text-muted">{label}</span>
      <span className="min-w-0 truncate text-text-secondary" title={value}>
        {value}
      </span>
    </div>
  );
}

export function ConnectionStatusButton() {
  const connected = useChatStore((s) => s.connected);
  const setConnected = useChatStore((s) => s.setConnected);
  const mode = useConnectionStore((s) => s.mode);
  const status = useConnectionStore((s) => s.status);
  const headCommit = useConnectionStore((s) => s.headCommit);
  const setConnectionStatus = useConnectionStore((s) => s.setStatus);
  const setLocalReady = useConnectionStore((s) => s.setLocalReady);
  const poll = useConnectionDiagnosticsStore((s) => s.poll);
  const browserSync = useConnectionDiagnosticsStore((s) => s.browserSync);
  const recordPollFailure = useConnectionDiagnosticsStore((s) => s.recordPollFailure);
  const recordBrowserSyncEvent = useConnectionDiagnosticsStore(
    (s) => s.recordBrowserSyncEvent,
  );
  const [refreshing, setRefreshing] = useState(false);
  const [retrying, setRetrying] = useState(false);

  const title = connected
    ? "Connected"
    : poll.lastError ?? browserSync.lastError ?? "Disconnected";
  const lastError = poll.lastError ?? browserSync.lastError;

  async function handleRefresh() {
    setRefreshing(true);
    try {
      const res = await client.health(AbortSignal.timeout(3000));
      if (!res.ok) {
        recordPollFailure("transport", res.error ?? "Health check failed");
      }
    } catch (error) {
      recordPollFailure("transport", error);
    } finally {
      setRefreshing(false);
    }
  }

  async function handleRetrySync() {
    setRetrying(true);
    try {
      const res = await client.retryBrowserSync();
      if (!res.ok) {
        recordBrowserSyncEvent({
          status: "error",
          error: res.error ?? "Browser sync failed",
        });
      }
    } catch (error) {
      recordBrowserSyncEvent({ status: "error", error: String(error) });
    } finally {
      setRetrying(false);
    }
  }

  function handleReconnect() {
    setConnected(false);
    if (mode === "local") setLocalReady(false);
    setConnectionStatus("disconnected");
  }

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label="Connection diagnostics"
          title={title}
          className="inline-flex size-5 shrink-0 items-center justify-center rounded-full hover:bg-surface/80 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-accent/50"
        >
          <span
            className={[
              "inline-block size-2 rounded-full",
              connected
                ? "bg-success shadow-[0_0_6px_var(--color-glow-success)]"
                : "bg-error",
            ].join(" ")}
          />
        </button>
      </PopoverTrigger>
      <PopoverContent
        align="start"
        className="w-[320px] max-w-[calc(100vw-1rem)] border-border bg-card p-3"
      >
        <div className="space-y-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <p className="text-sm font-semibold text-foreground">
                Connection diagnostics
              </p>
              <p className="mt-0.5 text-xs text-text-muted">
                {connected ? "Connected" : "Disconnected"} · {modeLabel(mode)}
              </p>
            </div>
            <span
              className={[
                "mt-1 inline-block size-2 rounded-full",
                connected ? "bg-success" : "bg-error",
              ].join(" ")}
            />
          </div>

          <div className="space-y-1.5 rounded-md border border-border bg-background p-2">
            <DiagnosticsRow label="App poll" value={connected ? "Healthy" : "Failing"} />
            <DiagnosticsRow label="Mode" value={modeLabel(mode)} />
            <DiagnosticsRow label="Runtime" value={status} />
            <DiagnosticsRow
              label="Last ok"
              value={formatTime(poll.lastSuccessAt)}
            />
            <DiagnosticsRow
              label="Commit"
              value={shortCommit(headCommit ?? poll.lastCommit)}
            />
            <DiagnosticsRow
              label="Failures"
              value={`transport ${poll.consecutiveTransportFailures}, workspace ${poll.consecutiveWorkspaceFailures}`}
            />
          </div>

          {mode === "local" ? (
            <div className="space-y-1.5 rounded-md border border-border bg-background p-2">
              <DiagnosticsRow
                label="Browser sync"
                value={browserSyncLabel(browserSync.status)}
              />
              <DiagnosticsRow
                label="CORS proxy"
                value={browserSync.corsProxy ?? "Unknown"}
              />
              <DiagnosticsRow
                label="Remote"
                value={browserSync.remoteUrl ?? "Unknown"}
              />
              <DiagnosticsRow
                label="Sync event"
                value={formatTime(browserSync.lastEventAt)}
              />
            </div>
          ) : null}

          {lastError ? (
            <div className="rounded-md border border-error/30 bg-error/10 p-2 text-xs text-error">
              <p className="line-clamp-4 break-words">{lastError}</p>
              <p className="mt-1 text-text-muted">
                Last error {formatTime(poll.lastErrorAt ?? browserSync.lastErrorAt)}
              </p>
            </div>
          ) : null}

          <div className="flex flex-wrap justify-end gap-1.5">
            <Button
              type="button"
              variant="outline"
              size="xs"
              onClick={handleRefresh}
              disabled={refreshing}
            >
              <RefreshCcw className={refreshing ? "animate-spin" : undefined} />
              Refresh
            </Button>
            {mode === "local" ? (
              <Button
                type="button"
                variant="outline"
                size="xs"
                onClick={handleRetrySync}
                disabled={retrying}
              >
                <RotateCcw className={retrying ? "animate-spin" : undefined} />
                Retry sync
              </Button>
            ) : null}
            <Button
              type="button"
              variant="outline"
              size="xs"
              onClick={handleReconnect}
            >
              <Unplug />
              Reconnect
            </Button>
          </div>
        </div>
      </PopoverContent>
    </Popover>
  );
}
