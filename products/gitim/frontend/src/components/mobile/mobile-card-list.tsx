import { useMemo, useState } from "react";
import { useNavigate } from "react-router";
import { LayoutGrid, Clock, Hash, User } from "lucide-react";
import type { Card, CardStatus } from "../../lib/types";
import { cn } from "../../lib/utils";

const STATUS_CONFIG: Record<CardStatus, { label: string; bg: string; text: string }> = {
  todo: { label: "To do", bg: "bg-muted", text: "text-muted-foreground" },
  doing: { label: "Doing", bg: "bg-status-doing-bg", text: "text-status-doing" },
  done: { label: "Done", bg: "bg-status-done-bg", text: "text-status-done" },
};

const FILTERS: { key: CardStatus | "all"; label: string }[] = [
  { key: "all", label: "All" },
  { key: "todo", label: "To do" },
  { key: "doing", label: "Doing" },
  { key: "done", label: "Done" },
];

function formatRelative(iso: string): string {
  const m = iso.match(/^(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z$/);
  if (!m) return iso;
  const [, y, mo, d, h, mi, s] = m;
  const date = new Date(Date.UTC(Number(y), Number(mo) - 1, Number(d), Number(h), Number(mi), Number(s)));
  const diffSec = Math.floor((Date.now() - date.getTime()) / 1000);
  if (diffSec < 60) return "just now";
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)}m ago`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)}h ago`;
  if (diffSec < 30 * 86400) return `${Math.floor(diffSec / 86400)}d ago`;
  return `${mo}/${d}`;
}

function StatusPill({ status }: { status: CardStatus }) {
  const cfg = STATUS_CONFIG[status];
  return (
    <span className={cn("inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px] font-medium", cfg.bg, cfg.text)}>
      {cfg.label}
    </span>
  );
}

function CardRow({ card }: { card: Card }) {
  const navigate = useNavigate();
  const visibleLabels = card.labels.slice(0, 3);
  const overflow = card.labels.length - visibleLabels.length;

  return (
    <button
      onClick={() => navigate(`/cards/${card.channel}/${card.card_id}`)}
      className="w-full text-left rounded-xl border border-border bg-card p-3.5 active:scale-[0.98] transition-transform"
    >
      <div className="flex items-start justify-between gap-2 mb-2">
        <h3 className="text-[15px] font-medium leading-snug line-clamp-2 flex-1 text-foreground">
          {card.title}
        </h3>
        <StatusPill status={card.status} />
      </div>

      <div className="flex items-center gap-3 text-[11px] text-text-muted mb-2">
        <span className="flex items-center gap-1">
          <Hash className="size-3" />
          {card.channel}
        </span>
        {card.assignee && (
          <span className="flex items-center gap-1">
            <User className="size-3" />
            @{card.assignee}
          </span>
        )}
      </div>

      {visibleLabels.length > 0 && (
        <div className="flex flex-wrap gap-1 mb-2">
          {visibleLabels.map((l) => (
            <span key={l} className="rounded-md border border-border bg-surface px-1.5 py-0.5 text-[10px] text-text-muted">
              {l}
            </span>
          ))}
          {overflow > 0 && <span className="text-[10px] text-text-muted">+{overflow}</span>}
        </div>
      )}

      <div className="flex items-center gap-1 text-[11px] text-text-faint">
        <Clock className="size-3" />
        {formatRelative(card.updated_at)}
      </div>
    </button>
  );
}

interface MobileCardListProps {
  cards: Card[];
}

export function MobileCardList({ cards }: MobileCardListProps) {
  const [filterStatus, setFilterStatus] = useState<CardStatus | "all">("all");

  const filtered = useMemo(() => {
    let list = [...cards];
    if (filterStatus !== "all") {
      list = list.filter((c) => c.status === filterStatus);
    }
    return list.sort((a, b) => b.updated_at.localeCompare(a.updated_at));
  }, [cards, filterStatus]);

  const counts = useMemo(() => {
    const c = { all: cards.length, todo: 0, doing: 0, done: 0 };
    for (const card of cards) {
      c[card.status]++;
    }
    return c;
  }, [cards]);

  return (
    <div className="flex-1 flex flex-col overflow-hidden">
      <div className="shrink-0 px-3 pt-3 pb-2 border-b border-border/60">
        <div className="flex items-center gap-1.5 overflow-x-auto no-scrollbar">
          {FILTERS.map((f) => (
            <button
              key={f.key}
              onClick={() => setFilterStatus(f.key)}
              className={cn(
                "shrink-0 px-3 py-1.5 rounded-full text-xs font-medium transition-colors",
                filterStatus === f.key
                  ? "bg-primary/15 text-primary"
                  : "bg-surface text-text-muted hover:text-foreground"
              )}
            >
              {f.label}
              <span className={cn("ml-1 font-mono", filterStatus === f.key ? "text-primary/70" : "text-text-faint")}>
                {counts[f.key]}
              </span>
            </button>
          ))}
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-3 py-3 space-y-2.5">
        {filtered.length === 0 ? (
          <div className="flex flex-col items-center justify-center gap-3 py-16">
            <div className="w-12 h-12 rounded-2xl bg-surface flex items-center justify-center border border-border">
              <LayoutGrid className="size-6 text-primary" />
            </div>
            <div className="text-center">
              <p className="text-sm font-medium text-foreground">No cards</p>
              <p className="text-xs text-text-muted mt-1">
                {filterStatus === "all" ? "No cards in this workspace yet" : `No ${filterStatus} cards`}
              </p>
            </div>
          </div>
        ) : (
          filtered.map((card) => <CardRow key={`${card.channel}/${card.card_id}`} card={card} />)
        )}
      </div>
    </div>
  );
}
