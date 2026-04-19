import { useEffect, useState, type ReactNode } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { ConnectForm } from "./connect-form";
import { InstallStep } from "./install-step";
import { SetupShell } from "./setup-shell";

interface SetupGateProps {
  children: ReactNode;
}

/**
 * Blocks the app shell until the runtime is reachable.
 *
 * Workspace provisioning is handled downstream by `App` via
 * `useWorkspaceStore`: if the runtime has zero workspaces, the app shows
 * a first-run "create your first workspace" screen; otherwise the user
 * switches between workspaces via the `WorkspaceSwitcher` in the header.
 */
export function SetupGate({ children }: SetupGateProps) {
  const status = useConnectionStore((s) => s.status);
  const port = useConnectionStore((s) => s.port);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);

  // First-time users land on the Install step; returning users (saved port)
  // skip straight to Connect. Once the user clicks "continue" we remember it
  // within this session so going back to edit the port does not re-show Install.
  const [installAcknowledged, setInstallAcknowledged] = useState(
    () => port != null,
  );

  // On mount: if we have a stored port, try to connect automatically.
  useEffect(() => {
    if (status !== "checking") return;
    if (!port) {
      setStatus("disconnected");
      return;
    }

    let cancelled = false;

    async function tryConnect() {
      try {
        const res = await fetch(`http://127.0.0.1:${port}/health`, {
          signal: AbortSignal.timeout(3000),
        });
        const data = await res.json();
        if (cancelled) return;

        if (data.service === "gitim-runtime") {
          if (data.version) setRuntimeVersion(data.version as string);
          setStatus("ready");
        } else {
          setStatus("disconnected");
        }
      } catch {
        if (!cancelled) setStatus("disconnected");
      }
    }

    tryConnect();
    return () => { cancelled = true; };
  }, [status, port, setStatus, setRuntimeVersion]);

  if (status === "checking") {
    return (
      <SetupShell
        step={2}
        title="Connect Runtime"
        description="Link GitIM·Cell to your local runtime daemon"
        loading
      />
    );
  }

  if (status === "disconnected") {
    if (!installAcknowledged) {
      return <InstallStep onContinue={() => setInstallAcknowledged(true)} />;
    }
    return <ConnectForm onBack={() => setInstallAcknowledged(false)} />;
  }

  return <>{children}</>;
}
