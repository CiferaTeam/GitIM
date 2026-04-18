import { useCallback, useEffect, useMemo, useState } from "react";
import { Plus } from "lucide-react";
import { useSearchParams } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  useCardStore,
  selectAllLabels,
  selectFilteredCards,
  sortByUpdatedDesc,
} from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import * as client from "@/lib/client";
import type { Card, CardFilter, CardStatus } from "@/lib/types";
import { CardFilterBar, EMPTY_CARD_FILTER, type CardFilterState } from "./card-filter-bar";
import { CardKanbanColumn } from "./card-kanban-column";
import { CardCreateDialog } from "./card-create-dialog";

function readFilterFromURL(params: URLSearchParams): CardFilterState {
  const assignee = params.get("assignee");
  return {
    channels: params.getAll("channel"),
    labels: params.getAll("label"),
    // Canonical "my cards" form is assignee=__me__; both the toggle and the
    // URL bind to the same field to keep a single source of truth.
    assignee: assignee === "__me__" ? null : assignee,
    mineOnly: assignee === "__me__",
  };
}

function writeFilterToURL(filter: CardFilterState): URLSearchParams {
  const p = new URLSearchParams();
  for (const ch of filter.channels) p.append("channel", ch);
  for (const l of filter.labels) p.append("label", l);
  if (filter.mineOnly) {
    p.set("assignee", "__me__");
  } else if (filter.assignee) {
    p.set("assignee", filter.assignee);
  }
  return p;
}

export function CardKanban() {
  const [searchParams, setSearchParams] = useSearchParams();
  const cards = useCardStore((s) => s.cards);
  const archivedCards = useCardStore((s) => s.archivedCards);
  const showArchived = useCardStore((s) => s.showArchived);
  const setCards = useCardStore((s) => s.setCards);
  const upsertCard = useCardStore((s) => s.upsertCard);
  const markCardInFlight = useCardStore((s) => s.markCardInFlight);
  const unmarkCardInFlight = useCardStore((s) => s.unmarkCardInFlight);
  const allLabels = useCardStore(selectAllLabels);
  const currentUser = useChatStore((s) => s.currentUser);

  const filter: CardFilterState = useMemo(
    () => readFilterFromURL(searchParams),
    [searchParams],
  );

  const [createOpen, setCreateOpen] = useState(false);

  // Refetch on mount
  useEffect(() => {
    (async () => {
      const res = await client.listCards();
      if (res.ok && res.data) setCards(res.data.cards);
    })();
  }, [setCards]);

  const handleFilterChange = useCallback(
    (next: CardFilterState) => {
      setSearchParams(writeFilterToURL(next));
    },
    [setSearchParams],
  );

  const filteredCards = useMemo(() => {
    const cf: CardFilter = {
      channels: filter.channels.length > 0 ? filter.channels : undefined,
      labels: filter.labels.length > 0 ? filter.labels : undefined,
      assignee: filter.mineOnly
        ? "__me__"
        : filter.assignee ?? undefined,
    };
    return sortByUpdatedDesc(selectFilteredCards(cards, cf, currentUser));
  }, [cards, filter, currentUser]);

  const filteredArchivedCards = useMemo(() => {
    if (!showArchived) return [];
    const cf: CardFilter = {
      channels: filter.channels.length > 0 ? filter.channels : undefined,
      labels: filter.labels.length > 0 ? filter.labels : undefined,
      assignee: filter.mineOnly
        ? "__me__"
        : filter.assignee ?? undefined,
    };
    return sortByUpdatedDesc(selectFilteredCards(archivedCards, cf, currentUser));
  }, [archivedCards, showArchived, filter, currentUser]);

  const byStatus = useMemo(() => {
    const g: Record<CardStatus, Card[]> = { todo: [], doing: [], done: [] };
    for (const c of filteredCards) g[c.status].push(c);
    return g;
  }, [filteredCards]);

  const archivedByStatus = useMemo(() => {
    const g: Record<CardStatus, Card[]> = { todo: [], doing: [], done: [] };
    for (const c of filteredArchivedCards) g[c.status].push(c);
    return g;
  }, [filteredArchivedCards]);

  const handleStatusChange = useCallback(
    async (card: Card, newStatus: CardStatus) => {
      const prev = card;
      // Mark in-flight before optimistic upsert so any interleaving poll
      // tick sees the guard before it can observe the optimistic state.
      markCardInFlight(card.channel, card.card_id);
      upsertCard({ ...card, status: newStatus });
      const res = await client.updateCard(card.channel, card.card_id, {
        status: newStatus,
      });
      unmarkCardInFlight(card.channel, card.card_id);
      if (!res.ok) {
        // Rollback
        upsertCard(prev);
        toast.error(`Failed to update: ${res.error ?? "unknown"}`);
      }
    },
    [upsertCard, markCardInFlight, unmarkCardInFlight],
  );

  const hasFilter =
    filter.channels.length > 0 ||
    filter.labels.length > 0 ||
    !!filter.assignee ||
    filter.mineOnly;

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="flex items-center justify-between px-4 py-3 border-b border-border">
        <h1 className="text-xl font-semibold">Cards</h1>
        <Button size="sm" onClick={() => setCreateOpen(true)} className="gap-1.5">
          <Plus className="h-4 w-4" />
          New card
        </Button>
      </div>

      <CardFilterBar
        value={filter}
        onChange={handleFilterChange}
        labelSuggestions={allLabels}
      />

      {cards.length === 0 && filteredArchivedCards.length === 0 ? (
        <EmptyState
          title="No cards yet"
          hint="Create a card from any channel or with the button above."
        />
      ) : filteredCards.length === 0 && filteredArchivedCards.length === 0 ? (
        <EmptyState
          title="No cards match these filters"
          hint="Try clearing a filter."
          onClear={hasFilter ? () => handleFilterChange(EMPTY_CARD_FILTER) : undefined}
        />
      ) : (
        <div className="flex-1 overflow-hidden p-4 flex gap-4">
          <CardKanbanColumn
            status="todo"
            cards={byStatus.todo}
            archivedCards={archivedByStatus.todo}
            onStatusChange={handleStatusChange}
          />
          <CardKanbanColumn
            status="doing"
            cards={byStatus.doing}
            archivedCards={archivedByStatus.doing}
            onStatusChange={handleStatusChange}
          />
          <CardKanbanColumn
            status="done"
            cards={byStatus.done}
            archivedCards={archivedByStatus.done}
            onStatusChange={handleStatusChange}
          />
        </div>
      )}

      <CardCreateDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
      />
    </div>
  );
}

function EmptyState({
  title,
  hint,
  onClear,
}: {
  title: string;
  hint: string;
  onClear?: () => void;
}) {
  return (
    <div className="flex-1 flex flex-col items-center justify-center gap-2 p-8">
      <p className="text-base font-medium">{title}</p>
      <p className="text-sm text-muted-foreground">{hint}</p>
      {onClear && (
        <Button variant="outline" size="sm" onClick={onClear} className="mt-2">
          Clear all filters
        </Button>
      )}
    </div>
  );
}
