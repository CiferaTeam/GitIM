import type { ReactNode } from "react";
import { Outlet, useNavigate } from "react-router";
import { HelpCircle } from "lucide-react";
import { TwitterXIcon } from "../icons/twitter-x";
import { ThemeToggle } from "../theme/theme-toggle";
import { useChatStore } from "../../hooks/use-chat-store";
import { HandlerName } from "../chat/handler-name";
import { WorkspaceSwitcher } from "../workspace/workspace-switcher";
import { UpdateIndicator } from "../update-indicator";
import { UsageIndicator } from "../usage-indicator";
import { TimezoneToggle } from "../timezone-toggle";
import { DonateDialog } from "../donate-dialog";
import { MobileTabBar } from "../mobile/mobile-tab-bar";
import { NavTabs } from "./nav-tabs";
import { ConnectionStatusButton } from "./connection-status-button";

interface AppShellProps {
  children?: ReactNode;
}

export function AppShell({ children }: AppShellProps) {
  const navigate = useNavigate();
  const currentUser = useChatStore((s) => s.currentUser);

  return (
    <div
      className="h-screen flex flex-col overflow-hidden bg-background text-foreground"
      style={{ height: "100dvh" }}
    >
      {/* Top bar */}
      <header className="h-12 border-b border-border flex items-center px-2 sm:px-4 justify-between gap-2 shrink-0 bg-card/80 backdrop-blur-md shadow-[0_1px_0_rgba(0,0,0,0.2)]">
        {/* Left: logo + workspace switcher + connection status */}
        <div className="flex flex-1 items-center gap-1.5 min-w-0 md:flex-none md:gap-2">
          <span className="font-bold text-sm tracking-tight text-foreground shrink-0">
            gitim
          </span>
          <ConnectionStatusButton />
          <div className="ml-0.5 min-w-0 flex-1 md:ml-1 md:flex-none">
            <WorkspaceSwitcher />
          </div>
        </div>

        {/* Center: nav tabs — hidden on mobile */}
        <div className="hidden md:block">
          <NavTabs />
        </div>

        {/* Right: theme toggle + twitter + update indicator + help + current user */}
        <div className="flex shrink-0 items-center justify-end gap-1 md:gap-2 md:min-w-[140px]">
          <ThemeToggle />
          <TimezoneToggle />
          <a
            href="https://x.com/arknights60"
            target="_blank"
            rel="noopener noreferrer"
            title="Twitter / X"
            className="hidden md:flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
          >
            <TwitterXIcon className="size-4" />
          </a>
          <div className="hidden md:block">
            <DonateDialog />
          </div>
          <div className="hidden md:block">
            <UsageIndicator />
          </div>
          <div className="hidden md:block">
            <UpdateIndicator />
          </div>
          <button
            type="button"
            onClick={() => navigate("/docs")}
            title="Documentation"
            className="flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
          >
            <HelpCircle className="size-4" />
          </button>
          {currentUser ? (
            <span
              className="max-w-[22vw] truncate text-xs text-text-secondary bg-surface px-2 py-1 rounded-md border border-border md:max-w-none"
              title={`Human handler: @${currentUser}`}
            >
              <HandlerName handler={currentUser} />
            </span>
          ) : null}
        </div>
      </header>

      {/* Content */}
      <main className="min-h-0 flex-1 overflow-hidden">
        {children ?? <Outlet />}
      </main>
      <div
        aria-hidden="true"
        className="shrink-0 h-[calc(4rem+env(safe-area-inset-bottom))] md:hidden"
      />
      <MobileTabBar />
    </div>
  );
}
