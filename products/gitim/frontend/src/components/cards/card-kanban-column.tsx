import { useState } from "react";
import type { Card, CardStatus } from "@/lib/types";
import { cn } from "@/lib/utils";
import { CardKanbanCell } from "./card-kanban-cell";
import { CARD_DRAG_MIME, decodeCardDrag } from "./card-drag";

const DONE_INITIAL_LIMIT = 20;

const STATUS_LABEL: Record<CardStatus, string> = {
  todo: "To do",
  doing: "Doing",
  done: "Done",
};

export interface CardKanbanColumnProps {
  status: CardStatus;
  cards: Card[];
  /** Archived cards for this column, rendered muted below active cards. Empty when "Show archived" is off. */
  archivedCards?: Card[];
  onStatusChange: (card: Card, newStatus: CardStatus) => void;
  /** Called when a card is dropped from another column. Same-column drops are ignored upstream. */
  onCardDropped: (channel: string, cardId: string, newStatus: CardStatus) => void;
}

export function CardKanbanColumn({
  status,
  cards,
  archivedCards = [],
  onStatusChange,
  onCardDropped,
}: CardKanbanColumnProps) {
  const [showAllDone, setShowAllDone] = useState(false);
  const [dropActive, setDropActive] = useState(false);

  const shouldCollapse =
    status === "done" && !showAllDone && cards.length > DONE_INITIAL_LIMIT;
  const visible = shouldCollapse ? cards.slice(0, DONE_INITIAL_LIMIT) : cards;
  const hidden = cards.length - visible.length;

  const hasArchived = archivedCards.length > 0;
  const totalCount = cards.length + archivedCards.length;

  return (
    <div
      className={cn(
        "flex-1 min-w-0 flex flex-col bg-kanban-column-bg rounded-lg border border-border transition-colors",
        dropActive && "border-[#3b82f6] bg-[#1e2030]",
      )}
      onDragOver={(e) => {
        // Only react to our own card-drag payload. Without this we'd light up
        // the column for any random text/file drag from outside the page.
        if (!e.dataTransfer.types.includes(CARD_DRAG_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "move";
        if (!dropActive) setDropActive(true);
      }}
      onDragLeave={(e) => {
        // dragleave fires when crossing into child elements too; only clear
        // when the cursor leaves the column's bounding box.
        if (e.currentTarget.contains(e.relatedTarget as Node | null)) return;
        setDropActive(false);
      }}
      onDrop={(e) => {
        setDropActive(false);
        const raw = e.dataTransfer.getData(CARD_DRAG_MIME);
        if (!raw) return;
        e.preventDefault();
        const payload = decodeCardDrag(raw);
        if (!payload) return;
        // No same-status short-circuit here: from_status is frozen at dragstart
        // and may be stale if a poll updated the card mid-drag. The parent
        // re-checks against live store state.
        onCardDropped(payload.channel, payload.card_id, status);
      }}
    >
      <header className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-sm font-medium">{STATUS_LABEL[status]}</h2>
        <span className="text-xs text-muted-foreground">{totalCount}</span>
      </header>
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {cards.length === 0 && !hasArchived ? (
          <p className="text-xs text-muted-foreground/60 text-center py-4">
            No cards
          </p>
        ) : (
          <>
            {visible.map((card) => (
              <CardKanbanCell
                key={`${card.channel}/${card.card_id}`}
                card={card}
                onStatusChange={onStatusChange}
              />
            ))}
            {hidden > 0 && (
              <button
                onClick={() => setShowAllDone(true)}
                className="w-full text-xs text-muted-foreground hover:text-foreground py-1.5 rounded border border-dashed border-border hover:border-border-strong transition-colors"
              >
                Show all ({hidden} more)
              </button>
            )}
            {hasArchived && (
              <>
                {cards.length > 0 && (
                  <div className="pt-2 pb-1 text-[10px] uppercase tracking-wider text-text-faint text-center border-t border-border/60 mt-2">
                    Archived · {archivedCards.length}
                  </div>
                )}
                {archivedCards.map((card) => (
                  <CardKanbanCell
                    key={`archived-${card.channel}/${card.card_id}`}
                    card={card}
                    archived
                    onStatusChange={onStatusChange}
                  />
                ))}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
