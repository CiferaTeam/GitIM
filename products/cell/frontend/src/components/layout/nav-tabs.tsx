import { NavLink } from "react-router";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { getVisibleNavigationItems } from "./navigation-items";

export function NavTabs() {
  const mode = useConnectionStore((s) => s.mode);
  const items = getVisibleNavigationItems(mode, "desktop");

  return (
    <div className="flex items-center gap-1 bg-muted/50 p-1 rounded-lg border border-border/60">
      {items.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          className={({ isActive }) =>
            [
              "relative px-4 py-1.5 text-sm font-medium rounded-md transition-all duration-200",
              isActive
                ? "bg-primary text-white shadow-sm"
                : "text-muted-foreground hover:text-foreground hover:bg-surface/60",
            ].join(" ")
          }
        >
          {item.label}
        </NavLink>
      ))}
    </div>
  );
}
