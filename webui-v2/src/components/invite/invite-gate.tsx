import { useState, useEffect, type ReactNode } from "react";
import { verifyInviteCode } from "../../lib/cell-api";
import { getDeviceId } from "../../lib/device";
import { GuidePage } from "./guide-page";

const INVITE_CODE_KEY = "gitim:invite_code";
const INVITE_VERIFIED_KEY = "gitim:invite_verified";
const SETUP_COMPLETED_KEY = "gitim:setup_completed";

type GateStatus = "checking" | "need_code" | "need_setup" | "verified";

export function InviteGate({ children }: { children: ReactNode }) {
  const [status, setStatus] = useState<GateStatus>("checking");
  const [code, setCode] = useState("");
  const [error, setError] = useState("");
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (import.meta.env.DEV) {
      setStatus("verified");
      return;
    }
    const verified = localStorage.getItem(INVITE_VERIFIED_KEY);
    if (!verified) {
      setStatus("need_code");
      return;
    }
    const setupDone = localStorage.getItem(SETUP_COMPLETED_KEY);
    setStatus(setupDone ? "verified" : "need_setup");
  }, []);

  if (status === "checking") {
    return (
      <div className="flex items-center justify-center h-screen bg-background text-muted-foreground text-sm">
        Loading...
      </div>
    );
  }

  if (status === "verified") {
    return <>{children}</>;
  }

  if (status === "need_setup") {
    return (
      <GuidePage
        onComplete={() => {
          localStorage.setItem(SETUP_COMPLETED_KEY, "true");
          setStatus("verified");
        }}
      />
    );
  }

  // status === "need_code"
  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    const trimmed = code.trim();
    if (!trimmed) return;

    setError("");
    setLoading(true);

    const deviceId = getDeviceId();
    const result = await verifyInviteCode(trimmed, deviceId);

    setLoading(false);
    if (result.ok) {
      localStorage.setItem(INVITE_CODE_KEY, trimmed);
      localStorage.setItem(INVITE_VERIFIED_KEY, "true");
      setStatus("need_setup");
    } else {
      setError(result.error ?? "验证失败");
    }
  };

  return (
    <div className="flex items-center justify-center h-screen bg-background p-4">
      <div className="w-full max-w-sm space-y-6">
        <div className="text-center space-y-2">
          <h1 className="text-xl font-bold tracking-tight text-foreground">
            GitIM Cell
          </h1>
          <p className="text-sm text-muted-foreground">输入口诀以继续</p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <input
            type="text"
            value={code}
            onChange={(e) => setCode(e.target.value)}
            placeholder="你的口诀"
            maxLength={64}
            className="w-full h-9 px-3 rounded-md border border-input bg-background text-sm text-foreground placeholder:text-text-muted focus:outline-none focus:ring-1 focus:ring-ring"
            autoFocus
          />

          {error && <p className="text-xs text-error">{error}</p>}

          <button
            type="submit"
            disabled={!code.trim() || loading}
            className="w-full h-9 rounded-md bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50 transition-colors"
          >
            {loading ? "验证中..." : "进入"}
          </button>
        </form>
      </div>
    </div>
  );
}
