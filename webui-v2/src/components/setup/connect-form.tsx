import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { SetupShell } from "./setup-shell";

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
      setError("请输入有效端口号 (1-65535)");
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
        setError("连接成功，但服务不是 gitim-runtime");
        return;
      }

      setPort(p);
      setRuntimeVersion(data.version ?? null);
      setStatus("ready");
    } catch {
      setError(`无法连接 127.0.0.1:${p}，请确认 Runtime 已启动`);
    } finally {
      setChecking(false);
    }
  }

  return (
    <SetupShell
      step={1}
      title="Connect Runtime"
      description="Link GitIM·Cell to your local runtime daemon"
      error={error}
      loading={checking}
      footer={
        <>
          请先启动 Runtime：{" "}
          <code className="text-text-secondary bg-surface px-1.5 py-0.5 rounded">
            gitim-runtime --port 16868
          </code>
        </>
      }
    >
      <form onSubmit={handleSubmit} className="space-y-4">
        <div className="space-y-2">
          <label
            htmlFor="port-input"
            className="text-sm font-medium text-text-secondary"
          >
            Runtime 端口
          </label>
          <input
            id="port-input"
            data-testid="port-input"
            type="text"
            inputMode="numeric"
            value={input}
            onChange={(e) => setInput(e.target.value)}
            placeholder="16868"
            className="w-full h-10 px-3 rounded-lg border border-border bg-background text-sm font-mono placeholder:text-text-faint focus:outline-none focus:ring-2 focus:ring-ring/40 focus:border-ring/60 transition-all"
            autoFocus
          />
        </div>

        <button
          data-testid="connect-button"
          type="submit"
          disabled={checking || !input.trim()}
          className="w-full h-10 rounded-lg bg-primary text-primary-foreground text-sm font-semibold hover:bg-primary/90 disabled:opacity-50 disabled:cursor-not-allowed transition-colors shadow-lg shadow-primary/20"
        >
          {checking ? "连接中..." : "连接"}
        </button>
      </form>
    </SetupShell>
  );
}
