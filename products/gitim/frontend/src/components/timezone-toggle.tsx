import { Check, Clock3 } from "lucide-react";
import { useTimezoneStore } from "@/hooks/use-timezone";
import {
  DISPLAY_TIMEZONES,
  displayTimezoneOption,
} from "@/lib/timezone";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "./ui/dropdown-menu";

export function TimezoneToggle() {
  const timezone = useTimezoneStore((s) => s.timezone);
  const setTimezone = useTimezoneStore((s) => s.setTimezone);
  const current = displayTimezoneOption(timezone);

  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild title={`Timezone: ${current.label}`}>
        <button
          type="button"
          className="flex h-7 items-center justify-center gap-1 rounded-md px-1.5 text-text-muted transition-colors hover:bg-surface/60 hover:text-foreground"
        >
          <Clock3 className="size-4" />
          <span className="hidden font-mono text-[11px] tabular-nums lg:inline">
            {current.label}
          </span>
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-[190px]">
        {DISPLAY_TIMEZONES.map((option) => (
          <DropdownMenuItem
            key={option.value}
            onClick={() => setTimezone(option.value)}
            className="cursor-pointer"
          >
            <span className="w-11 shrink-0 font-mono text-xs">
              {option.label}
            </span>
            <span className="min-w-0 flex-1 truncate text-xs text-text-muted">
              {option.description}
            </span>
            {timezone === option.value && (
              <Check className="size-4 text-primary" />
            )}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
