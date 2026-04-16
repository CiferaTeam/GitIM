import { useEffect, type ReactNode } from "react";
import {
  useConnectionStore,
  type ConnectionStatus,
} from "../../hooks/use-connection-store";
import { ConnectForm } from "./connect-form";
import { GitProviderForm } from "./git-provider-form";
import { WorkspaceForm } from "./workspace-form";
import { LocalSetup } from "./local-setup";

interface SetupGateProps {
  children: ReactNode;
}

export function SetupGate({ children }: SetupGateProps) {
  const mode = useConnectionStore((s) => s.mode);
  const status = useConnectionStore((s) => s.status);
  const port = useConnectionStore((s) => s.port);
  const localReady = useConnectionStore((s) => s.localReady);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const setMode = useConnectionStore((s) => s.setMode);

  // On mount: if remote mode, try to connect automatically
  useEffect(() => {
    if (mode === "local") return;
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
          setStatus(data.initialized ? "ready" : "connected");
        } else {
          setStatus("disconnected");
        }
      } catch {
        if (!cancelled) setStatus("disconnected");
      }
    }

    tryConnect();
    return () => {
      cancelled = true;
    };
  }, [mode, status, port, setStatus, setRuntimeVersion]);

  // Local mode
  if (mode === "local") {
    if (localReady) return <>{children}</>;
    return <LocalSetup />;
  }

  // Remote mode
  const screens: Record<ConnectionStatus, ReactNode> = {
    checking: (
      <div className="flex items-center justify-center h-screen bg-background text-muted-foreground text-sm">
        Connecting...
      </div>
    ),
    disconnected: (
      <div className="flex flex-col items-center justify-center min-h-screen bg-background gap-4">
        <ConnectForm />
        <button
          onClick={() => setMode("local")}
          className="text-sm text-muted-foreground hover:text-foreground"
        >
          Use Local Mode (no server needed)
        </button>
      </div>
    ),
    connected: <WorkspaceForm />,
    workspace_set: <GitProviderForm />,
    ready: <>{children}</>,
  };

  return <>{screens[status]}</>;
}
