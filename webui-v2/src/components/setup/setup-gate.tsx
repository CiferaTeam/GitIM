import { useEffect, type ReactNode } from "react";
import {
  useConnectionStore,
  type ConnectionStatus,
} from "../../hooks/use-connection-store";
import { ConnectForm } from "./connect-form";
import { GithubSetupForm } from "./github-setup-form";
import { GitProviderForm } from "./git-provider-form";
import { WorkspaceForm } from "./workspace-form";

interface SetupGateProps {
  children: ReactNode;
}

export function SetupGate({ children }: SetupGateProps) {
  const status = useConnectionStore((s) => s.status);
  const port = useConnectionStore((s) => s.port);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);

  // On mount: if we have a stored port, try to connect automatically
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
          setStatus(data.initialized ? "ready" : "connected");
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

  const screens: Record<ConnectionStatus, ReactNode> = {
    checking: (
      <div className="flex items-center justify-center h-screen bg-background text-muted-foreground text-sm">
        Connecting...
      </div>
    ),
    disconnected: <ConnectForm />,
    connected: <WorkspaceForm />,
    workspace_set: <GitProviderForm />,
    github_setup: <GithubSetupForm />,
    ready: <>{children}</>,
  };

  return <>{screens[status]}</>;
}
