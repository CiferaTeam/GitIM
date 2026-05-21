import { useState } from "react";
import { useNavigate } from "react-router";
import { useTimezoneStore } from "@/hooks/use-timezone";
import type { Card, CardStatus } from "@/lib/types";
import { Badge } from "@/components/ui/badge";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { formatRelativeTimestamp } from "@/lib/timezone";
import { cn } from "@/lib/utils";
import { CARD_DRAG_MIME, encodeCardDrag } from "./card-drag";

const STATUS_CLASS: Record<CardStatus, string> = {
  todo: "bg-muted text-muted-foreground",
  doing: "bg-status-doing-bg text-status-doing",
  done: "bg-status-done-bg text-status-done",
};

const STATUSES: CardStatus[] = ["todo", "doing", "done"];

export interface CardKanbanCellProps {
  card: Card;
  /** Render the card in a muted "archived" style and hide the status dropdown. */
  archived?: boolean;
  onStatusChange: (card: Card, newStatus: CardStatus) => void;
}

export function CardKanbanCell({
  card,
  archived = false,
  onStatusChange,
}: CardKanbanCellProps) {
  const navigate = useNavigate();
  const timezone = useTimezoneStore((s) => s.timezone);
  const [dragging, setDragging] = useState(false);

  const visibleLabels = card.labels.slice(0, 3);
  const overflow = card.labels.length - visibleLabels.length;

  return (
    <div
      onClick={() => navigate(`/cards/${card.channel}/${card.card_id}`)}
      draggable={!archived}
      onDragStart={(e) => {
        if (archived) {
          e.preventDefault();
          return;
        }
        // Bail when the drag started on an interactive descendant (e.g. the
        // status dropdown trigger). Without this, a tiny pointer movement
        // while clicking the pill drags the card instead of opening the menu.
        if (
          e.target instanceof Element &&
          e.target.closest("[data-no-drag]")
        ) {
          e.preventDefault();
          return;
        }
        e.dataTransfer.effectAllowed = "move";
        e.dataTransfer.setData(
          CARD_DRAG_MIME,
          encodeCardDrag({
            channel: card.channel,
            card_id: card.card_id,
          }),
        );
        setDragging(true);
      }}
      onDragEnd={() => setDragging(false)}
      className={cn(
        "group rounded-md border border-border bg-kanban-card-bg hover:bg-kanban-card-hover p-3 cursor-pointer transition-colors flex flex-col gap-2",
        archived && "opacity-55 hover:opacity-75",
        !archived && "hover:cursor-grab active:cursor-grabbing",
        dragging && "opacity-40",
      )}
    >
      <div className="flex items-start justify-between gap-2">
        <h3 className="text-sm font-medium leading-snug line-clamp-2 flex-1">
          {card.title}
        </h3>
        {archived ? (
          // Small "Archived" pill replaces the status dropdown — quiet,
          // non-interactive. Click still falls through to the card detail.
          <span
            className="shrink-0 rounded-sm px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wider bg-muted text-muted-foreground"
          >
            Archived
          </span>
        ) : (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <button
                data-no-drag
                onClick={(e) => e.stopPropagation()}
                className={cn(
                  "shrink-0 rounded px-2 py-0.5 text-xs font-medium capitalize",
                  STATUS_CLASS[card.status],
                )}
              >
                {card.status}
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" onClick={(e) => e.stopPropagation()}>
              {STATUSES.map((s) => (
                <DropdownMenuItem
                  key={s}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (s !== card.status) onStatusChange(card, s);
                  }}
                  className={cn(
                    "capitalize",
                    s === card.status && "font-semibold",
                  )}
                >
                  {s}
                </DropdownMenuItem>
              ))}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>
      <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
        <span className="truncate">#{card.channel}</span>
        {card.assignee && (
          <span className="truncate font-mono">@{card.assignee}</span>
        )}
      </div>
      {visibleLabels.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {visibleLabels.map((l) => (
            <Badge key={l} variant="outline" className="text-[10px]">
              {l}
            </Badge>
          ))}
          {overflow > 0 && (
            <span className="text-[10px] text-muted-foreground">
              +{overflow}
            </span>
          )}
        </div>
      )}
      <div className="text-[11px] text-muted-foreground/80">
        {formatRelativeTimestamp(card.updated_at, timezone)}
      </div>
    </div>
  );
}
