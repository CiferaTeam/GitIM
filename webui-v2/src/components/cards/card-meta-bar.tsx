import { useMemo, useState } from "react";
import { ChevronDown } from "lucide-react";
import type { Card, CardStatus } from "@/lib/types";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { LabelChipInput } from "@/components/ui/label-chip-input";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useCardStore, selectAllLabels } from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { cn } from "@/lib/utils";

const STATUSES: CardStatus[] = ["todo", "doing", "done"];

const STATUS_CLASS: Record<CardStatus, string> = {
  todo: "bg-muted text-muted-foreground",
  doing: "bg-[#60a5fa18] text-[#60a5fa]",
  done: "bg-[#4ade8018] text-[#4ade80]",
};

export interface CardMetaBarProps {
  card: Card;
  /** Update patch. Should optimistic-update then call backend; rollback on fail. */
  onUpdate: (patch: {
    status?: CardStatus;
    labels?: string[];
    assignee?: string | null;
  }) => Promise<void>;
  /** When true, all meta edit controls (status, assignee, labels) render read-only. */
  disabled?: boolean;
}

export function CardMetaBar({ card, onUpdate, disabled = false }: CardMetaBarProps) {
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);
  const allLabels = useCardStore(selectAllLabels);

  const assigneeOptions = useMemo(() => {
    const set = new Set<string>([...users, ...agents.map((a) => a.id)]);
    return [...set].sort();
  }, [users, agents]);

  const [assigneeOpen, setAssigneeOpen] = useState(false);

  // Read-only rendering when disabled — keeps meta visible (status, assignee,
  // labels) so the user still sees what the card looked like, just without
  // the dropdown/popover affordances.
  if (disabled) {
    return (
      <div className="border-b border-border px-4 py-3 flex flex-col gap-2">
        <h1 className="text-lg font-semibold leading-tight">{card.title}</h1>
        <div className="flex flex-wrap items-center gap-2 text-xs">
          <span
            className={cn(
              "rounded px-2 py-0.5 font-medium capitalize",
              STATUS_CLASS[card.status],
            )}
          >
            {card.status}
          </span>
          <span className="text-muted-foreground">·</span>
          {card.assignee ? (
            <span className="font-mono text-foreground">@{card.assignee}</span>
          ) : (
            <span className="text-muted-foreground">Unassigned</span>
          )}
          {card.labels.length > 0 && (
            <>
              <span className="text-muted-foreground">·</span>
              <div className="flex flex-wrap gap-1">
                {card.labels.map((l) => (
                  <span
                    key={l}
                    className="rounded-sm px-1.5 py-0.5 text-[10px] bg-muted text-muted-foreground"
                  >
                    {l}
                  </span>
                ))}
              </div>
            </>
          )}
        </div>
        <p className="text-[11px] text-muted-foreground">
          created by @{card.created_by} · card id {card.card_id}
        </p>
      </div>
    );
  }

  return (
    <div className="border-b border-border px-4 py-3 flex flex-col gap-2">
      <h1 className="text-lg font-semibold leading-tight">{card.title}</h1>
      <div className="flex flex-wrap items-center gap-2 text-xs">
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <button
              className={cn(
                "rounded px-2 py-0.5 font-medium capitalize",
                STATUS_CLASS[card.status],
              )}
            >
              {card.status}
            </button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="start">
            {STATUSES.map((s) => (
              <DropdownMenuItem
                key={s}
                onClick={() => {
                  if (s !== card.status) onUpdate({ status: s });
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

        <span className="text-muted-foreground">·</span>

        <Popover open={assigneeOpen} onOpenChange={setAssigneeOpen}>
          <PopoverTrigger asChild>
            <Button variant="ghost" size="sm" className="h-6 gap-1 text-xs">
              {card.assignee ? (
                <span className="font-mono">@{card.assignee}</span>
              ) : (
                <span className="text-muted-foreground">Unassigned</span>
              )}
              <ChevronDown className="h-3 w-3" />
            </Button>
          </PopoverTrigger>
          <PopoverContent className="w-48 max-h-72 overflow-auto p-1" align="start">
            <button
              onClick={() => {
                setAssigneeOpen(false);
                if (card.assignee !== null) onUpdate({ assignee: null });
              }}
              className="w-full text-left px-2 py-1.5 text-xs rounded hover:bg-accent hover:text-accent-foreground"
            >
              Unassigned
            </button>
            <div className="h-px bg-border my-1" />
            {assigneeOptions.map((opt) => (
              <button
                key={opt}
                onClick={() => {
                  setAssigneeOpen(false);
                  if (card.assignee !== opt) onUpdate({ assignee: opt });
                }}
                className="w-full text-left px-2 py-1.5 text-xs rounded hover:bg-accent hover:text-accent-foreground"
              >
                @{opt}
              </button>
            ))}
          </PopoverContent>
        </Popover>

        <span className="text-muted-foreground">·</span>

        <div className="flex-1 min-w-[200px]">
          <LabelChipInput
            value={card.labels}
            onChange={(labels) => onUpdate({ labels })}
            suggestions={allLabels}
            allowCreate
            compact
            placeholder="Add label…"
          />
        </div>
      </div>
      <p className="text-[11px] text-muted-foreground">
        created by @{card.created_by} · card id {card.card_id}
      </p>
    </div>
  );
}
