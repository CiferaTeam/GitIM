import { useEffect } from "react";
import { useThemeStore, initThemeListener } from "../../hooks/use-theme";

const THEME_CLASSES = [
  "dark",
  "light",
  "cyberpunk",
  "pixel",
  "pink",
  "chinese",
];

interface ThemeProviderProps {
  children: React.ReactNode;
}

export function ThemeProvider({ children }: ThemeProviderProps) {
  const resolved = useThemeStore((s) => s.resolved);

  useEffect(() => {
    initThemeListener();
    const html = document.documentElement;
    THEME_CLASSES.forEach((c) => html.classList.remove(c));
    html.classList.add(resolved);
    html.setAttribute("data-theme", resolved);
  }, [resolved]);

  return <>{children}</>;
}
