import type { ReactNode } from "react";
import { Outlet } from "react-router";
import { useChatStore } from "../../hooks/use-chat-store";
import { NavTabs } from "./nav-tabs";

interface AppShellProps {
  children?: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const connected = useChatStore((s) => s.connected);
  const currentUser = useChatStore((s) => s.currentUser);

  return (
    <div className="h-screen flex flex-col bg-background text-foreground">
      {/* Top bar */}
      <header className="h-12 border-b border-border/60 flex items-center px-4 justify-between shrink-0 bg-background/95 backdrop-blur-sm">
        {/* Left: logo + connection status */}
        <div className="flex items-center gap-2.5 min-w-[120px]">
          <span className="font-bold text-sm tracking-tight">GitIM</span>
          <span
            className={[
              "inline-block w-1.5 h-1.5 rounded-full",
              connected ? "bg-green-500 shadow-[0_0_4px_hsl(142_76%_45%/0.5)]" : "bg-red-500",
            ].join(" ")}
            title={connected ? "Connected" : "Disconnected"}
          />
        </div>

        {/* Center: nav tabs */}
        <NavTabs />

        {/* Right: current user */}
        <div className="flex items-center justify-end min-w-[120px]">
          <span className="text-xs text-muted-foreground font-mono">
            {currentUser ? `@${currentUser}` : ""}
          </span>
        </div>
      </header>

      {/* Content */}
      <main className="flex-1 overflow-hidden">
        {children ?? <Outlet />}
      </main>
    </div>
  );
}
