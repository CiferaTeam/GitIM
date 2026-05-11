import { useCallback, useEffect, useRef, useState } from "react";
import * as client from "@/lib/client";
import type { CronTimelineEntry } from "@/lib/types";

interface UseCronTimelineResult {
  entries: CronTimelineEntry[];
  truncated: boolean;
  loading: boolean;
  error: string | null;
  refetch: () => void;
}

// Module-level frozen empty so the hook can emit a stable reference on the
// "no slug / no window / pre-init" path without churning React's
// useSyncExternalStore comparisons (see project memory:
// project_zustand_selector_pitfalls.md). Hooks consume this hook directly,
// so the stability win is at the component boundary, not in a zustand store.
const EMPTY_ENTRIES: CronTimelineEntry[] = Object.freeze(
  [] as CronTimelineEntry[],
) as CronTimelineEntry[];

interface CronTimelineState {
  key: string | null;
  entries: CronTimelineEntry[];
  truncated: boolean;
  error: string | null;
}

/**
 * Fetch /crons/timeline?from=<iso>&to=<iso> for the active workspace and
 * surface the merged past/future/missed entry list plus a truncation flag.
 *
 * Both `from` and `to` are RFC 3339 strings. Pass `undefined` for both to
 * let the runtime fall back to its month-of-now default (matches the
 * backend's `default_window_now`). Callers walking month-by-month should
 * always pass an explicit window so navigation triggers a fresh fetch.
 *
 * The hook re-runs whenever any of (slug, from, to) change. A stale request
 * that resolves after the user has navigated to a different window is
 * dropped via the abort signal — no zombie state writes.
 */
export function useCronTimeline(
  slug: string | null,
  from: string | undefined,
  to: string | undefined,
): UseCronTimelineResult {
  // `refreshNonce` exists so a manual `refetch()` can re-trigger the effect
  // without changing any of the (slug, from, to) inputs.
  const [refreshNonce, setRefreshNonce] = useState(0);
  const requestKey = slug ? `${slug}\0${from ?? ""}\0${to ?? ""}\0${refreshNonce}` : null;
  const [timelineState, setTimelineState] = useState<CronTimelineState>({
    key: null,
    entries: EMPTY_ENTRIES,
    truncated: false,
    error: null,
  });
  const isCurrentRequest = timelineState.key === requestKey;
  const entries = isCurrentRequest ? timelineState.entries : EMPTY_ENTRIES;
  const truncated = isCurrentRequest ? timelineState.truncated : false;
  const error = isCurrentRequest ? timelineState.error : null;
  const loading = requestKey !== null && !isCurrentRequest;
  const abortRef = useRef<AbortController | null>(null);

  useEffect(() => {
    if (!slug || requestKey === null) return;

    // Cancel any in-flight fetch from the previous (slug, from, to).
    if (abortRef.current) {
      abortRef.current.abort();
    }
    const controller = new AbortController();
    abortRef.current = controller;

    let cancelled = false;

    (async () => {
      const res = await client.getCronTimeline(slug, from, to, controller.signal);
      if (cancelled || controller.signal.aborted) return;
      if (!res.ok || !res.data) {
        setTimelineState({
          key: requestKey,
          entries: EMPTY_ENTRIES,
          truncated: false,
          error: res.error ?? "Failed to load cron timeline",
        });
        return;
      }
      setTimelineState({
        key: requestKey,
        entries: res.data.entries,
        truncated: res.data.truncated === true,
        error: null,
      });
    })().catch((err: unknown) => {
      if (cancelled || controller.signal.aborted) return;
      // Aborts manifest as DOMException on real fetch; on jsdom mocks they
      // can also surface as plain Errors with name === "AbortError". Either
      // way, drop the result.
      if (err instanceof DOMException && err.name === "AbortError") return;
      if (err instanceof Error && err.name === "AbortError") return;
      setTimelineState({
        key: requestKey,
        entries: EMPTY_ENTRIES,
        truncated: false,
        error: err instanceof Error ? err.message : String(err),
      });
    });

    return () => {
      cancelled = true;
      controller.abort();
    };
  }, [slug, from, to, requestKey]);

  const refetch = useCallback(() => {
    setRefreshNonce((n) => n + 1);
  }, []);

  return { entries, truncated, loading, error, refetch };
}
