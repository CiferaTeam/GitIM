import type { ReactNode } from "react";
import { Outlet } from "react-router";
import { useChatStore } from "../../hooks/use-chat-store";
import { WorkspaceSwitcher } from "../workspace/workspace-switcher";
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
      <header className="h-12 border-b border-border flex items-center px-4 justify-between shrink-0 bg-card/80 backdrop-blur-md shadow-[0_1px_0_rgba(0,0,0,0.2)]">
        {/* Left: logo + workspace switcher + connection status */}
        <div className="flex items-center gap-2 min-w-0">
          <span className="font-bold text-sm tracking-tight text-foreground shrink-0">
            GitIM<span className="text-primary">·</span>Cell
          </span>
          <span
            className={[
              "inline-block w-2 h-2 rounded-full shrink-0",
              connected
                ? "bg-success shadow-[0_0_6px_rgba(74,222,128,0.6)]"
                : "bg-error",
            ].join(" ")}
            title={connected ? "Connected" : "Disconnected"}
          />
          <div className="ml-1 min-w-0">
            <WorkspaceSwitcher />
          </div>
        </div>

        {/* Center: nav tabs */}
        <NavTabs />

        {/* Right: current user */}
        <div className="flex items-center justify-end min-w-[140px]">
          {currentUser ? (
            <span className="text-xs text-text-secondary font-mono bg-surface px-2 py-1 rounded-md border border-border">
              @{currentUser}
            </span>
          ) : null}
        </div>
      </header>

      {/* Content */}
      <main className="flex-1 overflow-hidden">
        {children ?? <Outlet />}
      </main>
    </div>
  );
}
