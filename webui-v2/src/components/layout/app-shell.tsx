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
      <header className="h-14 border-b flex items-center px-4 justify-between shrink-0">
        {/* Left: logo + connection status */}
        <div className="flex items-center gap-2 min-w-[120px]">
          <span className="font-bold text-sm">GitIM</span>
          <span
            className={[
              "inline-block w-2 h-2 rounded-full",
              connected ? "bg-green-500" : "bg-red-500",
            ].join(" ")}
            title={connected ? "Connected" : "Disconnected"}
          />
        </div>

        {/* Center: nav tabs */}
        <NavTabs />

        {/* Right: current user */}
        <div className="flex items-center justify-end min-w-[120px]">
          <span className="text-sm text-muted-foreground">
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
