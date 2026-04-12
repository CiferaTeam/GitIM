import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

export function ConnectForm() {
  const port = useConnectionStore((s) => s.port);
  const error = useConnectionStore((s) => s.error);
  const setPort = useConnectionStore((s) => s.setPort);
  const setStatus = useConnectionStore((s) => s.setStatus);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const setError = useConnectionStore((s) => s.setError);

  const [input, setInput] = useState(port?.toString() ?? "");
  const [checking, setChecking] = useState(false);

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const p = parseInt(input, 10);
    if (!Number.isFinite(p) || p < 1 || p > 65535) {
      setError("Please enter a valid port (1-65535)");
      return;
    }

    setChecking(true);
    setError(null);

    try {
      const res = await fetch(`http://127.0.0.1:${p}/health`, {
        signal: AbortSignal.timeout(3000),
      });
      const data = await res.json();

      if (data.service !== "gitim-runtime") {
        setError("Connected, but service is not gitim-runtime");
        return;
      }

      setPort(p);
      setRuntimeVersion(data.version ?? null);
      setStatus("connected");
    } catch {
      setError(`Cannot reach runtime at port ${p}. Is it running?`);
    } finally {
      setChecking(false);
    }
  }

  return (
    <div className="flex flex-col items-center justify-center h-screen bg-background text-foreground">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-xl font-bold tracking-tight">GitIM</h1>
          <p className="text-sm text-muted-foreground">
            Connect to a running Runtime instance
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <label
              htmlFor="port-input"
              className="text-xs font-medium text-text-secondary"
            >
              Runtime Port
            </label>
            <input
              id="port-input"
              data-testid="port-input"
              type="text"
              inputMode="numeric"
              value={input}
              onChange={(e) => setInput(e.target.value)}
              placeholder="16868"
              className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm font-mono placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
              autoFocus
            />
          </div>

          {error && (
            <p data-testid="connect-error" className="text-xs text-error">
              {error}
            </p>
          )}

          <button
            data-testid="connect-button"
            type="submit"
            disabled={checking}
            className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
          >
            {checking ? "Connecting..." : "Connect"}
          </button>
        </form>

        <p className="text-xs text-text-muted text-center leading-relaxed">
          Start the runtime first:{" "}
          <code className="text-text-secondary">
            gitim-runtime --port 16868
          </code>
        </p>
      </div>
    </div>
  );
}
