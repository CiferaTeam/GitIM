import { MessageSquare, LayoutGrid } from "lucide-react";
import { useNavigate, useLocation } from "react-router";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { cn } from "../../lib/utils";

export function MobileTabBar() {
  const navigate = useNavigate();
  const location = useLocation();
  const mode = useConnectionStore((s) => s.mode);
  const path = location.pathname;

  const tabs =
    mode === "local"
      ? [{ key: "/chat", label: "Chat", icon: MessageSquare }]
      : [
          { key: "/chat", label: "Chat", icon: MessageSquare },
          { key: "/cards", label: "Cards", icon: LayoutGrid },
        ];

  return (
    <div className="shrink-0 h-16 border-t border-border bg-card/80 backdrop-blur-md flex items-center justify-around md:hidden">
      {tabs.map((t) => {
        const active =
          t.key === "/chat"
            ? path === "/chat" || path === "/"
            : path.startsWith(t.key);
        const Icon = t.icon;
        return (
          <button
            key={t.key}
            onClick={() => navigate(t.key)}
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
