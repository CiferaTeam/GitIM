import { NavLink } from "react-router";
import { useConnectionStore } from "../../hooks/use-connection-store";

export function NavTabs() {
  const mode = useConnectionStore((s) => s.mode);

  return (
    <div className="flex items-center gap-1">
      {mode === "remote" && (
        <NavLink
          to="/management"
          className={({ isActive }) =>
            [
              "px-4 py-1.5 text-sm font-medium rounded-md transition-colors",
              isActive
                ? "bg-accent text-accent-foreground"
                : "text-muted-foreground hover:text-foreground hover:bg-accent/50",
            ].join(" ")
          }
        >
          Agents
        </NavLink>
      )}
      <NavLink
        to="/chat"
        className={({ isActive }) =>
          [
            "px-4 py-1.5 text-sm font-medium rounded-md transition-colors",
            isActive
              ? "bg-accent text-accent-foreground"
              : "text-muted-foreground hover:text-foreground hover:bg-accent/50",
          ].join(" ")
        }
      >
        Chat
      </NavLink>
    </div>
  );
}
