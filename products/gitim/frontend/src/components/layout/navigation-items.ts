import {
  Bot,
  CalendarClock,
  ClipboardList,
  LayoutGrid,
  MessageSquare,
  type LucideIcon,
} from "lucide-react";
import type { ConnectionMode } from "@/hooks/use-connection-store";

export type NavigationSurface = "desktop" | "mobile";

export interface NavigationItem {
  to: string;
  label: string;
  icon: LucideIcon;
  requiresRuntime?: boolean;
  mobileHidden?: boolean;
}

// Crons require the runtime: the daemon owns the spec store and the
// runtime exposes the timeline endpoint. Browser mode has no cron engine,
// so the tab stays hidden there. Desktop-only mirrors Agents — the calendar
// grid doesn't fit a mobile tab bar in a meaningful way for v1.
const navItems: NavigationItem[] = [
  { to: "/management", label: "Agents", icon: Bot, requiresRuntime: true, mobileHidden: true },
  { to: "/chat", label: "Chat", icon: MessageSquare },
  { to: "/cards", label: "Cards", icon: LayoutGrid },
  { to: "/boards", label: "Boards", icon: ClipboardList },
  { to: "/crons", label: "周期任务", icon: CalendarClock, requiresRuntime: true, mobileHidden: true },
];

export function getVisibleNavigationItems(
  mode: ConnectionMode,
  surface: NavigationSurface,
): NavigationItem[] {
  return navItems.filter((item) => {
    if (item.requiresRuntime && mode !== "remote") return false;
    if (item.mobileHidden && surface === "mobile") return false;
    return true;
  });
}
