import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { getUUID } from "../lib/device";
import { checkVersion } from "../lib/cell-api";
import { health, updateAndRestart } from "../lib/client";
import { useConnectionStore } from "./use-connection-store";

// Human-readable messages for structured error codes from the backend updater.
// Codes sourced from `crates/gitim-runtime/src/update.rs::error_codes` — keep
// this map in sync when new codes are added there. Codes not in this map fall
// back to the raw `detail` field or a generic message.
const UPDATE_ERROR_MESSAGES: Record<string, string> = {
  sha_mismatch:
    "校验失败：下载的安装包哈希与官方发布不一致，已拒绝安装。请稍后重试或联系维护者。",
  sha_line_missing:
    "校验失败：SHA256SUMS 未列出当前平台，疑似 release 不完整。请查看 Release 页面。",
  download_failed: "下载失败，请检查网络后重试。",
  extract_failed: "解压失败，请查看后端日志。",
};

function friendlyUpdateError(
  errorCode: string | undefined,
  detail: string | undefined,
): string {
  if (errorCode && UPDATE_ERROR_MESSAGES[errorCode]) {
    return UPDATE_ERROR_MESSAGES[errorCode];
  }
  if (detail) return `升级失败：${detail}`;
  if (errorCode) return `升级失败：${errorCode}`;
  return "升级失败，请查看后端日志。";
}

const ONE_HOUR_MS = 60 * 60 * 1000;
// Restart-window poll cadence. Short enough to feel responsive; the child
// process re-binds the port in well under a second after parent exit.
const RESTART_POLL_MS = 500;
// Hard ceiling on the restart window before we give up and surface an error.
const RESTART_TIMEOUT_MS = 30_000;
// Per-poll fetch cap. The parent-exit → child-bind gap is ~100-500ms in
// practice, but TCP SYN retries on a just-closed socket can stall fetch for
// seconds. Abort each poll's request at 2s so we don't starve subsequent
// polls when the old process is tearing down.
const POLL_FETCH_TIMEOUT_MS = 2_000;

// `/health` returns the bare CARGO_PKG_VERSION ("0.4.2") while the update
// endpoint's `target_version` comes from the GitHub tag ("v0.4.2"). Strict
// string equality across these two representations would never match — and
// the restart-success detection loop would silently time out. Normalize both
// sides before comparing.
const stripV = (s: string): string => s.replace(/^v/, "");

interface VersionCheckResult {
  current: string | null;
  latest: string | null;
  hasUpdate: boolean;
  isUpdating: boolean;
  isRestarting: boolean;
  error: string | null;
  triggerUpdate: () => Promise<void>;
}

// Parse "X.Y.Z" (with optional leading `v`) into a triple. Returns null on
// any malformed input — callers should fail closed, matching the backend's
// `gitim-updater::is_newer` semantics.
function parseVersion(s: string | null): [number, number, number] | null {
  if (!s) return null;
  const cleaned = s.replace(/^v/, "").trim();
  const parts = cleaned.split(".");
  if (parts.length !== 3) return null;
  const nums = parts.map((p) => {
    const n = parseInt(p, 10);
    return Number.isFinite(n) && n >= 0 ? n : NaN;
  });
  if (nums.some(Number.isNaN)) return null;
  return nums as [number, number, number];
}

// Strict "latest is newer than current" — returns false for equal, older,
// or malformed inputs. Fail-closed behavior mirrors the runtime updater.
function isNewer(current: string | null, latest: string | null): boolean {
  const c = parseVersion(current);
  const l = parseVersion(latest);
  if (!c || !l) return false;
  for (let i = 0; i < 3; i++) {
    if (l[i] > c[i]) return true;
    if (l[i] < c[i]) return false;
  }
  return false;
}

/**
 * Combines runtime `/health` with the Cell API `check-version` endpoint to
 * drive the self-update state machine (Task 8).
 *
 * Polling cadence:
 *   - On mount: one immediate check
 *   - Every hour: background refresh of both current and latest
 *
 * `triggerUpdate()` drives the restart window: POST update-and-restart →
 * poll /health at 500ms until the version matches `target_version` → reload
 * the page so all stores / subscriptions re-establish. A 30s timeout flips
 * to error state.
 *
 * Page reload (vs granular refetch) is intentional: after a runtime restart
 * every open channel/DM/card subscription, every cached workspace list, and
 * every zustand store is potentially stale. A hard reload is the simplest
 * reliable reset; a surgical invalidation pass is a future optimization.
 */
export function useVersionCheck(): VersionCheckResult {
  const [latest, setLatest] = useState<string | null>(null);

  // Current runtime version is the source of truth in the store — we just
  // read it here so the connection probe + periodic refresh share one slot.
  const current = useConnectionStore((s) => s.runtimeVersion);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const isUpdating = useConnectionStore((s) => s.isUpdating);
  const isRestarting = useConnectionStore((s) => s.isRestarting);
  const updateError = useConnectionStore((s) => s.updateError);
  const setIsUpdating = useConnectionStore((s) => s.setIsUpdating);
  const setIsRestarting = useConnectionStore((s) => s.setIsRestarting);
  const setUpdateError = useConnectionStore((s) => s.setUpdateError);

  // Guard against overlapping restart polls and StrictMode double-fire.
  const restartingRef = useRef(false);
  // Tracks the currently-scheduled poll timer so unmount can tear it down.
  const pollHandleRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // --- Refresh primitives ----------------------------------------------------

  const refreshCurrent = useCallback(async () => {
    const res = await health();
    if (res.ok && res.data) {
      const version = (res.data as { version?: string }).version ?? null;
      if (version) setRuntimeVersion(version);
    }
  }, [setRuntimeVersion]);

  const refreshLatest = useCallback(async () => {
    const uuid = getUUID();
    const res = await checkVersion(uuid);
    if (res.ok && res.latest_version) {
      setLatest(res.latest_version);
    }
  }, []);

  // --- Periodic version check ------------------------------------------------

  useEffect(() => {
    let cancelled = false;

    async function tick() {
      if (cancelled) return;
      await Promise.all([refreshCurrent(), refreshLatest()]);
    }

    void tick();
    const handle = setInterval(tick, ONE_HOUR_MS);
    return () => {
      cancelled = true;
      clearInterval(handle);
    };
  }, [refreshCurrent, refreshLatest]);

  // --- Unmount cleanup for restart-window poll -------------------------------

  useEffect(
    () => () => {
      if (pollHandleRef.current !== null) {
        clearTimeout(pollHandleRef.current);
        pollHandleRef.current = null;
      }
    },
    [],
  );

  // --- Update trigger --------------------------------------------------------

  const triggerUpdate = useCallback(async () => {
    if (restartingRef.current) return;
    restartingRef.current = true;
    setUpdateError(null);

    const accept = await updateAndRestart();
    if (!accept.ok || !accept.data) {
      restartingRef.current = false;
      // `already_latest` is an info signal, not a failure: our "latest"
      // state was stale. Refresh silently so hasUpdate corrects itself.
      if (accept.error_code === "already_latest") {
        toast.info("Already up to date");
        await Promise.all([refreshCurrent(), refreshLatest()]);
        return;
      }
      const msg = friendlyUpdateError(accept.error_code, accept.error);
      setUpdateError(msg);
      toast.error(msg);
      return;
    }

    const targetVersion = accept.data.target_version;
    setIsUpdating(true);

    const startedAt = Date.now();
    let sawDisconnect = false;

    // Recursive setTimeout instead of setInterval: ensures a single in-flight
    // poll at a time. With setInterval, a fetch that stalls past the 500ms
    // cadence (e.g. TCP SYN retry against a just-closed old socket) would pile
    // concurrent callbacks on top of each other, and a delayed resolution
    // could race the success branch after cleanup had already fired. Stepping
    // one at a time keeps the state transitions linear.
    await new Promise<void>((resolve) => {
      const finish = () => {
        if (pollHandleRef.current !== null) {
          clearTimeout(pollHandleRef.current);
          pollHandleRef.current = null;
        }
        restartingRef.current = false;
        resolve();
      };

      const poll = async () => {
        // Timeout — couldn't confirm the new version came up.
        if (Date.now() - startedAt > RESTART_TIMEOUT_MS) {
          setIsUpdating(false);
          setIsRestarting(false);
          setUpdateError("升级可能失败,请手动重启 Runtime。");
          toast.error("升级可能失败,请手动重启 Runtime。");
          finish();
          return;
        }

        // Per-poll abort: caps this fetch so a stalled request doesn't eat
        // the polling budget. `health()` throws on abort; the catch below
        // treats it identically to a network failure.
        const ac = new AbortController();
        const abortTimer = setTimeout(() => ac.abort(), POLL_FETCH_TIMEOUT_MS);
        let res: Awaited<ReturnType<typeof health>>;
        try {
          res = await health(ac.signal);
        } catch {
          res = { ok: false, error: "fetch failed" };
        } finally {
          clearTimeout(abortTimer);
        }

        if (!res.ok) {
          // Connection down = parent process exiting. Child will rebind shortly.
          if (!sawDisconnect) {
            sawDisconnect = true;
            setIsRestarting(true);
          }
          pollHandleRef.current = setTimeout(poll, RESTART_POLL_MS);
          return;
        }

        const version = (res.data as { version?: string } | undefined)?.version ?? null;
        if (version && stripV(version) === stripV(targetVersion)) {
          setIsUpdating(false);
          setIsRestarting(false);
          setUpdateError(null);
          setRuntimeVersion(version);
          toast.success(`Updated to v${stripV(version)}`);
          finish();
          // Hard reload: reset every store and re-subscribe all live channels.
          // See hook docstring for rationale.
          setTimeout(() => window.location.reload(), 250);
          return;
        }
        // Health came back but version doesn't match yet (could be the old
        // process still answering). Keep polling.
        pollHandleRef.current = setTimeout(poll, RESTART_POLL_MS);
      };

      void poll();
    });
  }, [
    refreshCurrent,
    refreshLatest,
    setIsRestarting,
    setIsUpdating,
    setRuntimeVersion,
    setUpdateError,
  ]);

  const hasUpdate = isNewer(current, latest);

  return {
    current,
    latest,
    hasUpdate,
    isUpdating,
    isRestarting,
    error: updateError,
    triggerUpdate,
  };
}
