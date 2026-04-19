import { useEffect, useMemo, useState } from "react";
import { Plus, X } from "lucide-react";
import { useNavigate } from "react-router";
import { Button } from "@/components/ui/button";
import {
  useCardStore,
  sortByUpdatedDesc,
} from "@/hooks/use-card-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { Card, CardStatus } from "@/lib/types";
import { cn } from "@/lib/utils";
import { CardCreateDialog } from "./card-create-dialog";

const STATUS_DOT: Record<CardStatus, string> = {
  todo: "bg-muted-foreground/50",
  doing: "bg-[#60a5fa]",
  done: "bg-[#4ade80]",
};

export interface ChannelCardDrawerProps {
  channel: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

export function ChannelCardDrawer({
  channel,
  open,
  onOpenChange,
}: ChannelCardDrawerProps) {
  const navigate = useNavigate();
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const cards = useCardStore((s) => s.cards);
  const setCards = useCardStore((s) => s.setCards);

  const [createOpen, setCreateOpen] = useState(false);

  const channelCards = useMemo(
    () => sortByUpdatedDesc(cards.filter((c) => c.channel === channel)),
    [cards, channel],
  );

  // Refetch listCards whenever drawer opens to ensure freshness
  useEffect(() => {
    if (!open || !activeSlug) return;
    (async () => {
      const res = await client.listCards(activeSlug);
      if (res.ok && res.data) setCards(res.data.cards);
    })();
  }, [open, activeSlug, setCards]);

  // ESC to close
  useEffect(() => {
    if (!open) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") onOpenChange(false);
    }
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onOpenChange]);

  if (!open) return null;

  return (
    <>
      {/* Overlay */}
      <div
        className="fixed inset-0 z-40 bg-black/30"
        onClick={() => onOpenChange(false)}
        aria-hidden
      />
      {/* Panel */}
      <aside
        className="fixed right-0 top-0 bottom-0 z-50 w-96 bg-[#1c1c1e] border-l border-border flex flex-col shadow-xl animate-in slide-in-from-right duration-200"
        role="dialog"
        aria-label={`Cards in #${channel}`}
      >
        <header className="flex items-center justify-between px-4 py-3 border-b border-border">
          <div className="flex items-baseline gap-2">
            <h2 className="text-sm font-semibold">Cards</h2>
            <span className="text-xs text-muted-foreground">
              in #{channel}
            </span>
          </div>
          <div className="flex items-center gap-1">
            <Button
              size="sm"
              onClick={() => setCreateOpen(true)}
              className="gap-1 h-7 text-xs"
            >
              <Plus className="h-3 w-3" />
              New
            </Button>
            <button
              onClick={() => onOpenChange(false)}
              className="p-1 rounded hover:bg-muted text-muted-foreground hover:text-foreground"
              aria-label="Close"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
        </header>

        <div className="flex-1 overflow-y-auto p-2">
          {channelCards.length === 0 ? (
            <p className="text-xs text-muted-foreground/70 text-center py-6">
              No cards in this channel yet.
            </p>
          ) : (
            <ul className="space-y-1">
              {channelCards.map((card) => (
                <CardRow
                  key={card.card_id}
                  card={card}
                  onOpen={() => {
                    onOpenChange(false);
                    navigate(`/cards/${card.channel}/${card.card_id}`);
                  }}
                />
              ))}
            </ul>
          )}
        </div>
      </aside>

      <CardCreateDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        presetChannel={channel}
      />
    </>
  );
}

function CardRow({ card, onOpen }: { card: Card; onOpen: () => void }) {
  return (
    <li>
      <button
        onClick={onOpen}
        className="w-full flex items-start gap-2 px-2 py-2 rounded hover:bg-muted/50 text-left"
      >
        <span
          className={cn(
            "mt-1.5 h-2 w-2 rounded-full shrink-0",
            STATUS_DOT[card.status],
          )}
          title={card.status}
        />
        <div className="flex-1 min-w-0">
          <p className="text-xs font-medium truncate">{card.title}</p>
          <p className="text-[11px] text-muted-foreground truncate">
            {card.assignee ? `@${card.assignee}` : "unassigned"}
            {card.labels.length > 0 && (
              <span className="ml-2">
                {card.labels.slice(0, 3).join(" · ")}
                {card.labels.length > 3 && ` +${card.labels.length - 3}`}
              </span>
            )}
          </p>
        </div>
      </button>
    </li>
  );
}
