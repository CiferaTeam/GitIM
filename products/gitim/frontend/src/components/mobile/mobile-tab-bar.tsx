import { useNavigate, useLocation } from "react-router";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { getVisibleNavigationItems } from "../layout/navigation-items";
import { cn } from "../../lib/utils";

export function MobileTabBar() {
  const navigate = useNavigate();
  const location = useLocation();
  const mode = useConnectionStore((s) => s.mode);
  const path = location.pathname;

  const tabs = getVisibleNavigationItems(mode, "mobile");

  return (
    <div className="shrink-0 h-[calc(4rem+env(safe-area-inset-bottom))] pb-[env(safe-area-inset-bottom)] border-t border-border bg-card/80 backdrop-blur-md flex items-center justify-around md:hidden">
      {tabs.map((t) => {
        const active =
          t.to === "/chat"
            ? path === "/chat" || path === "/"
            : path.startsWith(t.to);
        const Icon = t.icon;
        return (
          <button
            key={t.to}
            onClick={() => navigate(t.to)}
            className={cn(
              "flex flex-col items-center gap-0.5 px-4 py-1 rounded-lg transition-colors active:scale-95",
              active ? "text-primary" : "text-text-muted"
            )}
          >
            <Icon className="size-5" />
            <span className="text-[10px] font-medium">{t.label}</span>
          </button>
        );
      })}
    </div>
  );
}
