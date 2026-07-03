import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ExternalLink, LayoutGrid, Loader2, MessageSquare } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import {
  cardPathKey,
  selectCardMessagesByPath,
  selectCardById,
  useCardStore,
} from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { formatTimestamp, type Card, type Message } from "@/lib/types";
import { cn } from "@/lib/utils";
import { toApiChannel } from "@/lib/scope-name";
import {
  getCardPreviewReadQuery,
  selectCardPreviewMessages,
} from "./reference-preview-utils";

type LoadStatus = "idle" | "loading" | "ok" | "error";

export interface CardReference {
  channel: string;
  cardId: string;
  line?: number;
  label?: string;
}

export interface MessageReference {
  channel: string;
  line: number;
}

function shortCardId(cardId: string): string {
  return cardId.length <= 12 ? cardId : `${cardId.slice(0, 8)}...${cardId.slice(-3)}`;
}

function statusClass(status: Card["status"]): string {
  switch (status) {
    case "done":
      return "border-success/30 bg-success/10 text-success";
    case "doing":
      return "border-primary/30 bg-primary/10 text-primary";
    case "todo":
    default:
      return "border-border bg-muted/50 text-text-muted";
  }
}

function windowAround(messages: Message[], line?: number): Message[] {
  const realMessages = messages.filter((m) => m.type !== "event");
  if (!line) return realMessages.slice(-8);
  const idx = realMessages.findIndex((m) => m.line_number === line);
  if (idx === -1) return realMessages.slice(0, 11);
  return realMessages.slice(Math.max(0, idx - 5), idx + 6);
}

function MessageRows({
  messages,
  targetLine,
}: {
  messages: Message[];
  targetLine?: number;
}) {
  const timezone = useTimezoneStore((s) => s.timezone);
  if (messages.length === 0) {
    return <div className="py-3 text-xs text-text-muted">No messages.</div>;
  }
  return (
    <div className="space-y-1.5">
      {messages.map((msg) => (
        <div
          key={`${msg.line_number}-${msg.author}`}
          className={cn(
            "rounded-md border border-transparent px-2 py-1.5",
            targetLine === msg.line_number && "border-primary/30 bg-primary/10",
          )}
        >
          <div className="flex items-center gap-2 text-[11px] text-text-muted">
            <span className="font-mono">L{String(msg.line_number).padStart(6, "0")}</span>
            <span className="font-medium text-foreground/80">@{msg.author}</span>
            <span>{formatTimestamp(msg.timestamp, timezone)}</span>
          </div>
          <div className="mt-0.5 line-clamp-3 whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground/90">
            {msg.body}
          </div>
        </div>
      ))}
    </div>
  );
}

export function CardReferenceLink({
  reference,
  onOpen,
  children,
  className,
}: {
  reference: CardReference;
  onOpen: () => void;
  children?: ReactNode;
  className?: string;
}) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const cachedCard = useCardStore((s) => selectCardById(s, reference.channel, reference.cardId));
  const cachedArchivedCard = useCardStore((s) =>
    s.archivedCards.find(
      (c) => c.channel === reference.channel && c.card_id === reference.cardId,
    ),
  );
  const upsertCard = useCardStore((s) => s.upsertCard);
  const upsertArchivedCard = useCardStore((s) => s.upsertArchivedCard);
  const messagePathKey = cardPathKey(reference.channel, reference.cardId);
  const cachedMessages = useCardStore(
    (s) => selectCardMessagesByPath(s, messagePathKey),
  );

  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<LoadStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [loadedCard, setLoadedCard] = useState<Card | null>(null);
  const [loadedMessages, setLoadedMessages] = useState<Message[]>([]);

  const card = loadedCard ?? cachedCard ?? cachedArchivedCard ?? null;
  const display = card?.title ?? reference.label ?? shortCardId(reference.cardId);
  const messages = loadedMessages.length > 0 ? loadedMessages : cachedMessages;
  const visibleMessages = useMemo(
    () => selectCardPreviewMessages(messages, reference.line),
    [messages, reference.line],
  );

  useEffect(() => {
    if (!open || !activeSlug || status === "loading" || status === "ok") return;
    let cancelled = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("loading");
    setError(null);
    const query = getCardPreviewReadQuery(reference.line);
    void client
      .readCard(activeSlug, reference.channel, reference.cardId, query)
      .then((res) => {
        if (cancelled) return;
        if (!res.ok || !res.data) {
          setStatus("error");
          setError(res.error ?? "Failed to load card");
          return;
        }
        setLoadedCard(res.data.meta);
        setLoadedMessages(res.data.entries);
        if (res.data.archived) {
          upsertArchivedCard(res.data.meta);
        } else {
          upsertCard(res.data.meta);
        }
        setStatus("ok");
      });
    return () => {
      cancelled = true;
    };
  }, [
    activeSlug,
    open,
    reference.cardId,
    reference.channel,
    reference.line,
    status,
    upsertArchivedCard,
    upsertCard,
  ]);

  const handleOpen = useCallback(
    (event: React.MouseEvent) => {
      event.stopPropagation();
      onOpen();
    },
    [onOpen],
  );

  return (
    <HoverCard open={open} onOpenChange={setOpen} openDelay={180} closeDelay={140}>
      <HoverCardTrigger asChild>
        <button
          type="button"
          className={cn(
            "inline-flex max-w-full items-center gap-1 rounded-md px-1 py-0.5 text-primary hover:bg-primary/10 hover:underline",
            className,
          )}
          onClick={handleOpen}
          title={`#${reference.channel}/${reference.cardId}`}
        >
          {children ?? (
            <>
              <LayoutGrid className="h-3 w-3 shrink-0" />
              <span className="truncate">{display}</span>
              {reference.line && (
                <span className="font-mono text-[11px] text-primary/70">
                  L{String(reference.line).padStart(6, "0")}
                </span>
              )}
            </>
          )}
        </button>
      </HoverCardTrigger>
      <HoverCardContent align="start" className="w-[360px] max-w-[calc(100vw-24px)] p-0">
        <div className="max-h-[360px] overflow-y-auto p-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold text-foreground">
                {display}
              </div>
              <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-text-muted">
                <span>#{reference.channel}</span>
                <span className="font-mono">{reference.cardId}</span>
              </div>
            </div>
            <Button variant="ghost" size="xs" onClick={handleOpen} className="gap-1">
              <ExternalLink className="h-3 w-3" />
              Open
            </Button>
          </div>

          {card && (
            <div className="mt-2 flex flex-wrap items-center gap-1.5">
              <span className={cn("rounded-md border px-1.5 py-0.5 text-[11px]", statusClass(card.status))}>
                {card.status}
              </span>
              <span className="rounded-md border border-border bg-muted/30 px-1.5 py-0.5 text-[11px] text-text-muted">
                {card.assignee ? `@${card.assignee}` : "unassigned"}
              </span>
            </div>
          )}

          <div className="mt-3 border-t border-border pt-3">
            {status === "loading" && visibleMessages.length === 0 ? (
              <div className="flex items-center gap-2 py-3 text-xs text-text-muted">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                Loading preview...
              </div>
            ) : status === "error" && visibleMessages.length === 0 ? (
              <div className="py-3 text-xs text-destructive">{error}</div>
            ) : (
              <MessageRows messages={visibleMessages} targetLine={reference.line} />
            )}
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
}

export function MessageReferenceLink({
  reference,
  onOpen,
}: {
  reference: MessageReference;
  onOpen: () => void;
}) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const currentMessages = useChatStore((s) => s.messages);
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<LoadStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [loadedMessages, setLoadedMessages] = useState<Message[]>([]);

  const messages = useMemo(() => {
    if (loadedMessages.length > 0) return loadedMessages;
    if (currentChannel !== reference.channel) return [];
    return windowAround(currentMessages, reference.line);
  }, [currentChannel, currentMessages, loadedMessages, reference.channel, reference.line]);

  useEffect(() => {
    if (!open || !activeSlug || status === "loading" || status === "ok") return;
    if (currentChannel === reference.channel && currentMessages.length > 0) {
      // eslint-disable-next-line react-hooks/set-state-in-effect
      setStatus("ok");
      return;
    }
    let cancelled = false;
    setStatus("loading");
    setError(null);
    const since = reference.line > 6 ? reference.line - 6 : undefined;
    void client
      .read(activeSlug, toApiChannel(reference.channel), 11, since)
      .then((res) => {
        if (cancelled) return;
        if (!res.ok || !res.data) {
          setStatus("error");
          setError(res.error ?? "Failed to load message");
          return;
        }
        setLoadedMessages(res.data.entries as Message[]);
        setStatus("ok");
      });
    return () => {
      cancelled = true;
    };
  }, [
    activeSlug,
    currentChannel,
    currentMessages.length,
    open,
    reference.channel,
    reference.line,
    status,
  ]);

  const handleOpen = useCallback(
    (event: React.MouseEvent) => {
      event.stopPropagation();
      onOpen();
    },
    [onOpen],
  );

  return (
    <HoverCard open={open} onOpenChange={setOpen} openDelay={180} closeDelay={140}>
      <HoverCardTrigger asChild>
        <button
          type="button"
          className="inline-flex items-center gap-1 rounded-md px-1 py-0.5 text-primary hover:bg-primary/10 hover:underline"
          onClick={handleOpen}
        >
          <MessageSquare className="h-3 w-3 shrink-0" />
          <span>#{reference.channel}:L{String(reference.line).padStart(6, "0")}</span>
        </button>
      </HoverCardTrigger>
      <HoverCardContent align="start" className="w-[360px] max-w-[calc(100vw-24px)] p-0">
        <div className="max-h-[360px] overflow-y-auto p-3">
          <div className="flex items-center justify-between gap-3">
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold text-foreground">
                #{reference.channel}
              </div>
              <div className="mt-1 font-mono text-[11px] text-text-muted">
                L{String(reference.line).padStart(6, "0")}
              </div>
            </div>
            <Button variant="ghost" size="xs" onClick={handleOpen} className="gap-1">
              <ExternalLink className="h-3 w-3" />
              Open
            </Button>
          </div>
          <div className="mt-3 border-t border-border pt-3">
            {status === "loading" && messages.length === 0 ? (
              <div className="flex items-center gap-2 py-3 text-xs text-text-muted">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                Loading preview...
              </div>
            ) : status === "error" && messages.length === 0 ? (
              <div className="py-3 text-xs text-destructive">{error}</div>
            ) : (
              <MessageRows messages={messages} targetLine={reference.line} />
            )}
          </div>
        </div>
      </HoverCardContent>
    </HoverCard>
  );
}
