import { Moon, Sun, Monitor, Check } from "lucide-react";
import { useThemeStore, type Theme } from "../../hooks/use-theme";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

const OPTIONS: { value: Theme; label: string; icon: React.ReactNode }[] = [
  { value: "light", label: "Light", icon: <Sun className="size-4" /> },
  { value: "dark", label: "Dark", icon: <Moon className="size-4" /> },
  { value: "system", label: "System", icon: <Monitor className="size-4" /> },
];

export function ThemeToggle() {
  const { theme, setTheme } = useThemeStore();

  const current = OPTIONS.find((o) => o.value === theme)!;

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        asChild
        title={`Theme: ${current.label}`}
      >
        <button
          type="button"
          className="flex items-center justify-center w-7 h-7 rounded-md text-text-muted hover:text-foreground hover:bg-surface/60 transition-colors"
        >
          {current.icon}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[140px]">
        {OPTIONS.map((option) => (
          <DropdownMenuItem
            key={option.value}
            onClick={() => setTheme(option.value)}
            className="cursor-pointer"
          >
            <span className="flex items-center justify-center w-4">
              {option.icon}
            </span>
            <span className="flex-1">{option.label}</span>
            {theme === option.value && (
              <Check className="size-4 text-primary" />
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
