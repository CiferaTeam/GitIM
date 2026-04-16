import { useState } from "react";
import { useConnectionStore } from "../../hooks/use-connection-store";

interface GuidePageProps {
  onComplete: () => void;
}

export function GuidePage({ onComplete }: GuidePageProps) {
  const setPort = useConnectionStore((s) => s.setPort);
  const setRuntimeVersion = useConnectionStore((s) => s.setRuntimeVersion);
  const setStatus = useConnectionStore((s) => s.setStatus);

  const [portInput, setPortInput] = useState("16868");
  const [connecting, setConnecting] = useState(false);
  const [connectError, setConnectError] = useState("");

  const handleConnect = async () => {
    const p = parseInt(portInput, 10);
    if (!Number.isFinite(p) || p < 1 || p > 65535) {
      setConnectError("请输入有效端口号 (1-65535)");
      return;
    }

    setConnecting(true);
    setConnectError("");

    try {
      const res = await fetch(`http://127.0.0.1:${p}/health`, {
        signal: AbortSignal.timeout(3000),
      });
      const data = await res.json();

      if (data.service !== "gitim-runtime") {
        setConnectError("连接成功，但服务不是 gitim-runtime");
        return;
      }

      // Persist port + version in connection store so SetupGate picks it up
      setPort(p);
      if (data.version) setRuntimeVersion(data.version as string);
      setStatus(data.initialized ? "ready" : "connected");

      onComplete();
    } catch {
      setConnectError(`无法连接 127.0.0.1:${p}，请确认 Runtime 已启动`);
    } finally {
      setConnecting(false);
    }
  };

  return (
    <div className="flex items-center justify-center min-h-screen bg-background p-4">
      <div className="w-full max-w-lg space-y-8">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-bold tracking-tight text-foreground">
            设置 GitIM·Cell
          </h1>
          <p className="text-sm text-muted-foreground">
            首次使用，请完成以下步骤
          </p>
        </div>

        {/* Step 1: Install */}
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-foreground">
            1. 安装 GitIM 本地组件
          </h2>
          <div className="rounded-md bg-card p-3 font-mono text-xs text-foreground leading-relaxed select-all">
            curl -sSf
            https://raw.githubusercontent.com/CiferaTeam/gitim-releases/main/install.sh
            | sh
          </div>
        </section>

        {/* Step 2: Start Runtime */}
        <section className="space-y-2">
          <h2 className="text-sm font-medium text-foreground">
            2. 启动 Runtime
          </h2>
          <div className="rounded-md bg-card p-3 font-mono text-xs text-foreground select-all">
            gitim-runtime --port 16868 --daemon
          </div>
          <p className="text-xs text-text-muted leading-relaxed">
            <span className="text-foreground font-mono">--port</span> 和{" "}
            <span className="text-foreground font-mono">--daemon</span>{" "}
            均为可选项。加{" "}
            <span className="text-foreground font-mono">-d</span>（或{" "}
            <span className="text-foreground font-mono">--daemon</span>
            ）后台运行；不加则前台运行，方便查看日志。
          </p>
          <p className="text-xs text-text-muted leading-relaxed">
            Runtime 在 24 小时无活动后会自动退出，无需手动关闭。
          </p>
        </section>

        {/* Step 3: Connect */}
        <section className="space-y-3">
          <h2 className="text-sm font-medium text-foreground">3. 连接</h2>
          <div className="flex gap-2">
            <input
              type="text"
              inputMode="numeric"
              value={portInput}
              onChange={(e) => setPortInput(e.target.value)}
              placeholder="16868"
              className="flex-1 h-9 px-3 rounded-md border border-input bg-background text-sm font-mono text-foreground placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
            />
            <button
              onClick={handleConnect}
              disabled={connecting}
              className="h-9 px-4 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
            >
              {connecting ? "连接中..." : "连接"}
            </button>
          </div>
          {connectError && (
            <p className="text-xs text-error">{connectError}</p>
          )}
        </section>

      </div>
    </div>
  );
}
