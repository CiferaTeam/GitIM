import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "dark" | "light" | "system";

interface ThemeState {
  theme: Theme;
  resolved: "dark" | "light";
  setTheme: (theme: Theme) => void;
}

function resolveTheme(theme: Theme): "dark" | "light" {
  if (theme !== "system") return theme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
}

export const useThemeStore = create<ThemeState>()(
  persist(
    (set) => ({
      theme: "system",
      resolved: resolveTheme("system"),
      setTheme: (theme) => set({ theme, resolved: resolveTheme(theme) }),
    }),
    {
      name: "gitim-theme",
      partialize: (state) => ({ theme: state.theme }),
      onRehydrateStorage: () => (state) => {
        if (state) {
          state.resolved = resolveTheme(state.theme);
        }
      },
    }
  )
);

// Sync resolved theme when system preference changes at runtime
let _listenerInit = false;
export function initThemeListener() {
  if (_listenerInit) return;
  _listenerInit = true;
  const mql = window.matchMedia("(prefers-color-scheme: dark)");
  mql.addEventListener("change", () => {
    const store = useThemeStore.getState();
    if (store.theme === "system") {
      store.setTheme("system");
    }
  });
}
