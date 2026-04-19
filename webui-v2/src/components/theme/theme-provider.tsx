import { useEffect } from "react";
import { useThemeStore, initThemeListener } from "../../hooks/use-theme";

interface ThemeProviderProps {
  children: React.ReactNode;
}

export function ThemeProvider({ children }: ThemeProviderProps) {
  const resolved = useThemeStore((s) => s.resolved);

  useEffect(() => {
    initThemeListener();
    const html = document.documentElement;
    html.classList.remove("dark", "light");
    html.classList.add(resolved);
    html.setAttribute("data-theme", resolved);
  }, [resolved]);

  return <>{children}</>;
}
