import { useCallback, useEffect, useRef, useState } from "react";
import { toast } from "sonner";
import { getUUID } from "../lib/device";
import { checkVersion } from "../lib/cell-api";
import { health, updateAndRestart } from "../lib/client";
import { useConnectionStore } from "./use-connection-store";

const ONE_HOUR_MS = 60 * 60 * 1000;
// Restart-window poll cadence. Short enough to feel responsive; the child
// process re-binds the port in well under a second after parent exit.
const RESTART_POLL_MS = 500;
// Hard ceiling on the restart window before we give up and surface an error.
const RESTART_TIMEOUT_MS = 30_000;

interface VersionCheckResult {
  current: string | null;
  latest: string | null;
  hasUpdate: boolean;
  isUpdating: boolean;
  isRestarting: boolean;
  error: string | null;
  triggerUpdate: () => Promise<void>;
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

  // --- Periodic version check ------------------------------------------------

  useEffect(() => {
    let cancelled = false;

    async function refreshCurrent() {
      const res = await health();
      if (cancelled) return;
      if (res.ok && res.data) {
        const version = (res.data as { version?: string }).version ?? null;
        if (version) setRuntimeVersion(version);
      }
    }

    async function refreshLatest() {
      const uuid = getUUID();
      const res = await checkVersion(uuid);
      if (cancelled) return;
      if (res.ok && res.latest_version) {
        setLatest(res.latest_version);
      }
    }

    async function tick() {
      await Promise.all([refreshCurrent(), refreshLatest()]);
    }

    void tick();
    const handle = setInterval(tick, ONE_HOUR_MS);
    return () => {
      cancelled = true;
      clearInterval(handle);
    };
  }, [setRuntimeVersion]);

  // --- Update trigger --------------------------------------------------------

  const triggerUpdate = useCallback(async () => {
    if (restartingRef.current) return;
    restartingRef.current = true;
    setUpdateError(null);

    const accept = await updateAndRestart();
    if (!accept.ok || !accept.data) {
      restartingRef.current = false;
      const msg = accept.error ?? "Failed to start update";
      setUpdateError(msg);
      toast.error(msg);
      return;
    }

    const targetVersion = accept.data.target_version;
    setIsUpdating(true);

    const startedAt = Date.now();
    let sawDisconnect = false;

    await new Promise<void>((resolve) => {
      const poll = setInterval(async () => {
        // Timeout — couldn't confirm the new version came up.
        if (Date.now() - startedAt > RESTART_TIMEOUT_MS) {
          clearInterval(poll);
          setIsUpdating(false);
          setIsRestarting(false);
          setUpdateError("Update may have failed, please restart manually");
          toast.error("Update may have failed, please restart manually");
          restartingRef.current = false;
          resolve();
          return;
        }

        const res = await health();
        if (!res.ok) {
          // Connection down = parent process exiting. Child will rebind shortly.
          if (!sawDisconnect) {
            sawDisconnect = true;
            setIsRestarting(true);
          }
          return;
        }

        const version = (res.data as { version?: string } | undefined)?.version ?? null;
        if (version && version === targetVersion) {
          clearInterval(poll);
          setIsUpdating(false);
          setIsRestarting(false);
          setUpdateError(null);
          setRuntimeVersion(version);
          toast.success(`Updated to v${version}`);
          restartingRef.current = false;
          // Hard reload: reset every store and re-subscribe all live channels.
          // See hook docstring for rationale.
          setTimeout(() => window.location.reload(), 250);
          resolve();
        }
        // else: health came back but version doesn't match yet (could be the
        // old process still answering). Keep polling.
      }, RESTART_POLL_MS);
    });
  }, [setIsRestarting, setIsUpdating, setRuntimeVersion, setUpdateError]);

  const hasUpdate =
    current != null && latest != null && current !== latest;

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
