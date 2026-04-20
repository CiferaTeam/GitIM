import type { ReactNode } from "react";
import { Outlet, useNavigate } from "react-router";
import { HelpCircle } from "lucide-react";
import { TwitterXIcon } from "../icons/twitter-x";
import { ThemeToggle } from "../theme/theme-toggle";
import { useChatStore } from "../../hooks/use-chat-store";
import { WorkspaceSwitcher } from "../workspace/workspace-switcher";
import { UpdateIndicator } from "../update-indicator";
import { UsageIndicator } from "../usage-indicator";
import { NavTabs } from "./nav-tabs";

interface AppShellProps {
  children?: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const navigate = useNavigate();
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
                ? "bg-success shadow-[0_0_6px_var(--color-glow-success)]"
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

        {/* Right: theme toggle + twitter + update indicator + help + current user */}
        <div className="flex items-center justify-end gap-2 min-w-[140px]">
          <ThemeToggle />
          <a
            href="https://x.com/arknights60"
            target="_blank"
            rel="noopener noreferrer"
            title="Twitter / X"
            className="flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
          >
            <TwitterXIcon className="size-4" />
          </a>
          <UsageIndicator />
          <UpdateIndicator />
          <button
            type="button"
            onClick={() => navigate("/docs")}
            title="Documentation"
            className="flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
          >
            <HelpCircle className="size-4" />
          </button>
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
