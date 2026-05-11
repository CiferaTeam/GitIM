import { X } from "lucide-react";
import { Button } from "@/components/ui/button";
import type { CronTimelineEntry } from "@/lib/types";

interface CronDayPanelProps {
  slug: string | null;
  dayKey: string | null;
  entries: CronTimelineEntry[] | null;
  onClose: () => void;
}

/**
 * Stub panel — fleshed out in Task 6.2. Renders a placeholder so the
 * calendar grid compiles and the day-selection contract is established
 * before the detail view + run viewer lands.
 */
export function CronDayPanel({ dayKey, entries, onClose }: CronDayPanelProps) {
  if (!dayKey) {
    return (
      <div className="flex h-full items-center justify-center p-6 text-sm text-muted-foreground">
        选中日历中的一天来查看详情
      </div>
    );
  }
  return (
    <div className="flex h-full flex-col">
      <header className="flex items-center justify-between border-b border-border px-4 py-3">
        <div>
          <h2 className="text-sm font-semibold">{dayKey}</h2>
          <p className="text-xs text-muted-foreground">
            {entries?.length ?? 0} 个任务（UTC）
          </p>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          onClick={onClose}
          aria-label="Close day panel"
        >
          <X className="size-4" />
        </Button>
      </header>
      <div className="flex-1 overflow-y-auto px-4 py-3 text-sm text-muted-foreground">
        详情视图将在 Task 6.2 中接入。
      </div>
    </div>
  );
}
