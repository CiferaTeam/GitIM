import { Bot, LayoutGrid, MessageSquare, type LucideIcon } from "lucide-react";
import type { ConnectionMode } from "@/hooks/use-connection-store";

export type NavigationSurface = "desktop" | "mobile";

export interface NavigationItem {
  to: string;
  label: string;
  icon: LucideIcon;
  requiresRuntime?: boolean;
  mobileHidden?: boolean;
}

const navItems: NavigationItem[] = [
  { to: "/management", label: "Agents", icon: Bot, requiresRuntime: true, mobileHidden: true },
  { to: "/chat", label: "Chat", icon: MessageSquare },
  { to: "/cards", label: "Cards", icon: LayoutGrid },
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
