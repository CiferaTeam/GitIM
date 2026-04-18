import { useState } from "react";
import type { Card, CardStatus } from "@/lib/types";
import { CardKanbanCell } from "./card-kanban-cell";

const DONE_INITIAL_LIMIT = 20;

const STATUS_LABEL: Record<CardStatus, string> = {
  todo: "To do",
  doing: "Doing",
  done: "Done",
};

export interface CardKanbanColumnProps {
  status: CardStatus;
  cards: Card[];
  onStatusChange: (card: Card, newStatus: CardStatus) => void;
}

export function CardKanbanColumn({
  status,
  cards,
  onStatusChange,
}: CardKanbanColumnProps) {
  const [showAllDone, setShowAllDone] = useState(false);

  const shouldCollapse =
    status === "done" && !showAllDone && cards.length > DONE_INITIAL_LIMIT;
  const visible = shouldCollapse ? cards.slice(0, DONE_INITIAL_LIMIT) : cards;
  const hidden = cards.length - visible.length;

  return (
    <div className="flex-1 min-w-0 flex flex-col bg-[#1c1c1e] rounded-lg border border-border">
      <header className="flex items-center justify-between px-3 py-2 border-b border-border">
        <h2 className="text-sm font-medium">{STATUS_LABEL[status]}</h2>
        <span className="text-xs text-muted-foreground">{cards.length}</span>
      </header>
      <div className="flex-1 overflow-y-auto p-2 space-y-2">
        {cards.length === 0 ? (
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
          </>
        )}
      </div>
    </div>
  );
}
