import { Archive, Copy, GripVertical, Loader2 } from "lucide-react";

import type { QuickSessionListItem } from "@/lib/types";
import { formatTimestamp } from "@/lib/types";
import { cn } from "@/lib/utils";
import { QUICK_SESSION_DRAG_MIME } from "@/lib/quick-session-ref";
import { useTimezoneStore } from "@/hooks/use-timezone";

interface QuickSessionListProps {
  items: QuickSessionListItem[];
  selectedId: string | null;
  loading: boolean;
  error: string | null;
  workspaceKey: string;
  onSelect: (id: string) => void;
  onCopy: (ref: string) => void;
}

export function QuickSessionList({
  items,
  selectedId,
  loading,
  error,
  workspaceKey,
  onSelect,
  onCopy,
}: QuickSessionListProps) {
  const timezone = useTimezoneStore((state) => state.timezone);
  if (loading && items.length === 0) {
    return (
      <div className="flex items-center gap-2 px-3 py-6 text-xs text-text-muted">
        <Loader2 className="size-3.5 animate-spin" />
        Loading sessions…
      </div>
    );
  }
  if (error && items.length === 0) {
    return <div className="px-3 py-6 text-xs text-destructive">{error}</div>;
  }
  if (items.length === 0) {
    return (
      <div className="px-3 py-6 text-center text-xs leading-relaxed text-text-muted">
        No Quick Sessions here yet.
      </div>
    );
  }

  return (
    <div className="min-h-0 flex-1 overflow-y-auto p-1.5" role="list">
      {items.map((item) => (
        <div
          key={item.id}
          role="listitem"
          draggable
          onDragStart={(event) => {
            event.dataTransfer.effectAllowed = "copy";
            event.dataTransfer.setData(
              QUICK_SESSION_DRAG_MIME,
              JSON.stringify({ ref: item.ref, workspaceKey }),
            );
            event.dataTransfer.setData("text/plain", item.ref);
          }}
          className={cn(
            "group mb-1 flex items-center rounded-lg border border-transparent transition-colors",
            selectedId === item.id
              ? "border-primary/30 bg-primary/10"
              : "hover:bg-surface/70",
          )}
        >
          <GripVertical className="ml-1 size-3.5 shrink-0 cursor-grab text-text-faint" />
          <button
            type="button"
            className="min-w-0 flex-1 px-2 py-2 text-left"
            onClick={() => onSelect(item.id)}
            aria-current={selectedId === item.id ? "true" : undefined}
          >
            <span className="flex items-center gap-1.5">
              <span className="truncate text-xs font-medium text-foreground">
                {item.title ?? "Untitled session"}
              </span>
              {item.archived ? (
                <Archive className="size-3 shrink-0 text-text-muted" />
              ) : null}
            </span>
            <span className="mt-0.5 flex items-center gap-1.5 text-[11px] text-text-muted">
              <span>@{item.agent_id}</span>
              <span>·</span>
              <span>{item.status}</span>
              <span>·</span>
              <span>{formatTimestamp(item.updated_at, timezone)}</span>
            </span>
            <span className="mt-0.5 block truncate text-[11px] text-text-secondary">
              {item.last_message_preview || "No preview yet"}
            </span>
            <span className="mt-0.5 block truncate font-mono text-[10px] text-text-faint" title={item.ref}>
              {item.ref}
            </span>
          </button>
          <button
            type="button"
            className="mr-1 rounded-md p-1.5 text-text-faint opacity-0 transition-colors hover:bg-primary/10 hover:text-primary focus:opacity-100 group-hover:opacity-100"
            onClick={(event) => {
              event.stopPropagation();
              onCopy(item.ref);
            }}
            aria-label={`Copy reference for ${item.title ?? item.id}`}
          >
            <Copy className="size-3.5" />
          </button>
        </div>
      ))}
    </div>
  );
}
