import { MessageSquare, LayoutGrid, Bot } from "lucide-react";
import { useNavigate, useLocation } from "react-router";
import { cn } from "../../lib/utils";

export function MobileTabBar() {
  const navigate = useNavigate();
  const location = useLocation();
  const path = location.pathname;

  const tabs = [
    { key: "/chat", label: "Chat", icon: MessageSquare },
    { key: "/cards", label: "Cards", icon: LayoutGrid },
    { key: "/management", label: "Agents", icon: Bot },
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
