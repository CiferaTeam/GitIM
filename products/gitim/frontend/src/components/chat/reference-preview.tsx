import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  Check,
  Copy,
  ExternalLink,
  LayoutGrid,
  Loader2,
  MessageCircleMore,
  MessageSquare,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  HoverCard,
  HoverCardContent,
  HoverCardTrigger,
} from "@/components/ui/hover-card";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import {
  cardPathKey,
  selectCardMessagesByPath,
  selectCardById,
  useCardStore,
} from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { usePopoverPin } from "@/hooks/use-popover-pin";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import {
  formatTimestamp,
  type Card,
  type Message,
  type QuickSessionDetail,
  type QuickSessionStatus,
} from "@/lib/types";
import { cn } from "@/lib/utils";
import {
  formatQuickSessionRef,
  QUICK_SESSION_DRAG_MIME,
} from "@/lib/quick-session-ref";
import { toApiChannel } from "@/lib/scope-name";
import { workspaceIdentity } from "@/lib/workspace-key";
import {
  getCardReplyPreviewReadQuery,
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

export interface QuickSessionReference {
  sessionId: string;
  line?: number;
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

function quickSessionStatusClass(status: QuickSessionStatus): string {
  switch (status) {
    case "active":
      return "border-success/30 bg-success/10 text-success";
    case "running":
    case "needs_title":
      return "border-primary/30 bg-primary/10 text-primary";
    case "error":
      return "border-destructive/30 bg-destructive/10 text-destructive";
    case "archived":
    default:
      return "border-border bg-muted/50 text-text-muted";
  }
}

function hasSelectedTextWithin(element: HTMLElement): boolean {
  const selection = window.getSelection();
  if (!selection || selection.isCollapsed || selection.toString().trim().length === 0) {
    return false;
  }
  return [selection.anchorNode, selection.focusNode].some(
    (node) => node !== null && element.contains(node),
  );
}

function MessagePreviewRow({
  message,
  targetLine,
  collapseLongBody,
  onMessageOpen,
}: {
  message: Message;
  targetLine?: number;
  collapseLongBody: boolean;
  onMessageOpen?: (line: number) => void;
}) {
  const timezone = useTimezoneStore((s) => s.timezone);
  const bodyId = useId();
  const bodyRef = useRef<HTMLSpanElement>(null);
  const [expanded, setExpanded] = useState(false);
  const [hasOverflow, setHasOverflow] = useState(false);
  const lineLabel = `L${String(message.line_number).padStart(6, "0")}`;

  const measureOverflow = useCallback(() => {
    const element = bodyRef.current;
    if (!element || !collapseLongBody || expanded) return;
    setHasOverflow(element.scrollHeight > element.clientHeight + 1);
  }, [collapseLongBody, expanded]);

  useLayoutEffect(() => {
    if (!collapseLongBody || !bodyRef.current) return;
    measureOverflow();

    const observer = typeof ResizeObserver === "undefined"
      ? null
      : new ResizeObserver(measureOverflow);
    observer?.observe(bodyRef.current);
    window.addEventListener("resize", measureOverflow);

    let cancelled = false;
    void document.fonts?.ready.then(() => {
      if (!cancelled) measureOverflow();
    });

    return () => {
      cancelled = true;
      observer?.disconnect();
      window.removeEventListener("resize", measureOverflow);
    };
  }, [collapseLongBody, measureOverflow, message.body]);

  const body = (
    <span
      ref={bodyRef}
      id={bodyId}
      className={cn(
        "mt-0.5 block whitespace-pre-wrap break-words text-xs leading-relaxed text-foreground/90",
        collapseLongBody
          ? !expanded && "line-clamp-4"
          : "line-clamp-6",
      )}
    >
      {message.body}
    </span>
  );
  const content = (
    <>
      <span className="flex items-center gap-2 text-[11px] text-text-muted">
        <span className="font-mono">{lineLabel}</span>
        <span className="font-medium text-foreground/80">@{message.author}</span>
        <span>{formatTimestamp(message.timestamp, timezone)}</span>
        {onMessageOpen ? (
          <ExternalLink
            aria-hidden="true"
            className="ml-auto size-3 shrink-0 opacity-0 transition-opacity group-hover/row:opacity-70 group-focus-within/row:opacity-70"
          />
        ) : null}
      </span>
      {body}
    </>
  );
  const rowClassName = cn(
    "group/row rounded-md border border-transparent px-2 py-1.5 transition-colors hover:bg-surface/60",
    targetLine === message.line_number && "border-primary/30 bg-primary/10",
  );

  if (!onMessageOpen) {
    return <div className={rowClassName}>{content}</div>;
  }

  return (
    <article className={rowClassName}>
      <button
        type="button"
        title={`Open full card at ${lineLabel}`}
        className="block w-full cursor-pointer select-text text-left outline-none focus-visible:rounded-sm focus-visible:ring-2 focus-visible:ring-primary/30"
        onClick={(event) => {
          event.stopPropagation();
          if (event.detail > 0 && hasSelectedTextWithin(event.currentTarget)) {
            return;
          }
          onMessageOpen(message.line_number);
        }}
      >
        {content}
        <span className="sr-only">Open {lineLabel} in the full card</span>
      </button>
      {collapseLongBody && hasOverflow ? (
        <button
          type="button"
          aria-controls={bodyId}
          aria-expanded={expanded}
          className="mt-1 rounded-sm text-[11px] font-medium text-primary/80 outline-none transition-colors hover:text-primary hover:underline focus-visible:ring-2 focus-visible:ring-primary/30"
          onClick={(event) => {
            event.stopPropagation();
            setExpanded((current) => !current);
          }}
        >
          {expanded ? "Show less" : "Show more"}
        </button>
      ) : null}
    </article>
  );
}

function MessageRows({
  messages,
  targetLine,
  collapseLongBodies = false,
  onMessageOpen,
}: {
  messages: Message[];
  targetLine?: number;
  collapseLongBodies?: boolean;
  onMessageOpen?: (line: number) => void;
}) {
  if (messages.length === 0) {
    return <div className="py-3 text-xs text-text-muted">No messages.</div>;
  }
  return (
    <div className="space-y-1.5">
      {messages.map((message) => (
        <MessagePreviewRow
          key={`${message.line_number}-${message.author}`}
          message={message}
          targetLine={targetLine}
          collapseLongBody={collapseLongBodies}
          onMessageOpen={onMessageOpen}
        />
      ))}
    </div>
  );
}

export function CardReferenceLink({
  reference,
  onOpen,
  children,
  className,
  latestReplyCount,
  previewStartLine,
}: {
  reference: CardReference;
  onOpen: (line?: number) => void;
  children?: ReactNode;
  className?: string;
  latestReplyCount?: number;
  previewStartLine?: number;
}) {
  const isLatestRepliesPreview = latestReplyCount != null;
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

  const [hoverOpen, setHoverOpen] = useState(false);
  const replyPopover = usePopoverPin();
  const open = isLatestRepliesPreview ? replyPopover.open : hoverOpen;
  const [status, setStatus] = useState<LoadStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [loadedCard, setLoadedCard] = useState<Card | null>(null);
  const [loadedMessages, setLoadedMessages] = useState<Message[]>([]);

  const card = loadedCard ?? cachedCard ?? cachedArchivedCard ?? null;
  const display = card?.title ?? reference.label ?? shortCardId(reference.cardId);
  const messages = loadedMessages.length > 0 ? loadedMessages : cachedMessages;
  const previewLine = isLatestRepliesPreview ? undefined : reference.line;
  const visibleMessages = useMemo(
    () => {
      const groupedMessages =
        isLatestRepliesPreview && previewStartLine != null
          ? messages.filter(
              (message) =>
                message.line_number >= previewStartLine &&
                (reference.line == null || message.line_number <= reference.line),
            )
          : messages;
      return selectCardPreviewMessages(groupedMessages, previewLine);
    },
    [
      isLatestRepliesPreview,
      messages,
      previewLine,
      previewStartLine,
      reference.line,
    ],
  );

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("idle");
    setError(null);
    setLoadedCard(null);
    setLoadedMessages([]);
  }, [
    activeSlug,
    latestReplyCount,
    previewStartLine,
    reference.cardId,
    reference.channel,
    reference.line,
  ]);

  useEffect(() => {
    if (!open || !activeSlug) return;
    let cancelled = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("loading");
    setError(null);
    const query =
      isLatestRepliesPreview && previewStartLine != null
        ? getCardReplyPreviewReadQuery(previewStartLine, latestReplyCount)
        : getCardPreviewReadQuery(previewLine);
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
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setStatus("error");
        setError(reason instanceof Error ? reason.message : "Failed to load card");
      });
    return () => {
      cancelled = true;
    };
  }, [
    activeSlug,
    isLatestRepliesPreview,
    latestReplyCount,
    open,
    previewLine,
    previewStartLine,
    reference.cardId,
    reference.channel,
    reference.line,
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

  const triggerContent = children ?? (
    <>
      <LayoutGrid className="h-3 w-3 shrink-0" />
      <span className="truncate">{display}</span>
      {reference.line ? (
        <span className="font-mono text-[11px] text-primary/70">
          L{String(reference.line).padStart(6, "0")}
        </span>
      ) : null}
    </>
  );
  const triggerClassName = cn(
    "inline-flex max-w-full items-center gap-1 rounded-md px-1 py-0.5 text-primary hover:bg-primary/10 hover:underline",
    className,
  );
  const previewPanel = (
    <div className="p-3">
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
          View all
        </Button>
      </div>

      {card ? (
        <div className="mt-2 flex flex-wrap items-center gap-1.5">
          <span
            className={cn(
              "rounded-md border px-1.5 py-0.5 text-[11px]",
              statusClass(card.status),
            )}
          >
            {card.status}
          </span>
          <span className="rounded-md border border-border bg-muted/30 px-1.5 py-0.5 text-[11px] text-text-muted">
            {card.assignee ? `@${card.assignee}` : "unassigned"}
          </span>
        </div>
      ) : null}

      <div className="mt-3 border-t border-border pt-3">
        <div className="mb-2 flex items-center justify-between gap-3 text-[11px]">
          <span className="font-medium text-text-secondary">
            {isLatestRepliesPreview ? "Recent discussion" : "Discussion"}
          </span>
          {isLatestRepliesPreview ? (
            <span className="text-text-muted">
              {latestReplyCount} new {latestReplyCount === 1 ? "reply" : "replies"}
            </span>
          ) : null}
        </div>
        {status === "loading" && visibleMessages.length === 0 ? (
          <div className="flex items-center gap-2 py-3 text-xs text-text-muted">
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
            Loading preview...
          </div>
        ) : status === "error" && visibleMessages.length === 0 ? (
          <div className="py-3 text-xs text-destructive">{error}</div>
        ) : (
          <div
            role="region"
            aria-label="Recent card discussion"
            className="max-h-[min(420px,60vh)] overflow-y-auto overscroll-contain rounded-md pr-1 outline-none focus-visible:ring-2 focus-visible:ring-primary/40"
            tabIndex={isLatestRepliesPreview ? 0 : undefined}
            onWheel={(event) => event.stopPropagation()}
            onPointerDown={(event) => event.stopPropagation()}
            onTouchStart={(event) => event.stopPropagation()}
          >
            <MessageRows
              messages={visibleMessages}
              targetLine={reference.line}
              collapseLongBodies={isLatestRepliesPreview}
              onMessageOpen={isLatestRepliesPreview ? onOpen : undefined}
            />
          </div>
        )}
      </div>
    </div>
  );

  if (isLatestRepliesPreview) {
    return (
      <Popover open={replyPopover.open} onOpenChange={replyPopover.handleOpenChange}>
        <div
          className="inline-flex max-w-full"
          onPointerEnter={replyPopover.openFromHover}
          onPointerLeave={replyPopover.scheduleHoverClose}
        >
          <PopoverTrigger asChild>
            <button
              type="button"
              aria-expanded={replyPopover.open}
              aria-haspopup="dialog"
              className={triggerClassName}
              onFocus={replyPopover.openFromFocus}
              onClick={(event) => {
                event.stopPropagation();
                replyPopover.handleTriggerClick(event);
              }}
              title={`Preview #${reference.channel}/${reference.cardId}`}
            >
              {triggerContent}
            </button>
          </PopoverTrigger>
        </div>
        <PopoverContent
          align="start"
          aria-label={`Replies for ${display}`}
          className="w-[460px] max-w-[calc(100vw-24px)] p-0"
          onOpenAutoFocus={replyPopover.handleOpenAutoFocus}
          onCloseAutoFocus={replyPopover.handleCloseAutoFocus}
          onPointerEnter={replyPopover.clearCloseTimer}
          onPointerLeave={replyPopover.scheduleHoverClose}
          onEscapeKeyDown={replyPopover.handleEscapeKeyDown}
        >
          {previewPanel}
        </PopoverContent>
      </Popover>
    );
  }

  return (
    <HoverCard
      open={hoverOpen}
      onOpenChange={setHoverOpen}
      openDelay={180}
      closeDelay={140}
    >
      <HoverCardTrigger asChild>
        <button
          type="button"
          className={triggerClassName}
          onClick={handleOpen}
          title={`#${reference.channel}/${reference.cardId}`}
        >
          {triggerContent}
        </button>
      </HoverCardTrigger>
      <HoverCardContent
        align="start"
        className="w-[460px] max-w-[calc(100vw-24px)] p-0"
      >
        {previewPanel}
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
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("idle");
    setError(null);
    setLoadedMessages([]);
  }, [activeSlug, reference.channel, reference.line]);

  useEffect(() => {
    if (!open || !activeSlug) return;
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
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setStatus("error");
        setError(reason instanceof Error ? reason.message : "Failed to load message");
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
      <HoverCardContent align="start" className="w-[420px] max-w-[calc(100vw-24px)] p-0">
        <div className="max-h-[420px] overflow-y-auto p-3">
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

export function QuickSessionReferenceLink({
  reference,
}: {
  reference: QuickSessionReference;
}) {
  const activeSlug = useWorkspaceStore((state) => state.activeSlug);
  const workspaces = useWorkspaceStore((state) => state.workspaces);
  const mode = useConnectionStore((state) => state.mode);
  const activeWorkspace = workspaces.find((workspace) => workspace.slug === activeSlug);
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;
  const [open, setOpen] = useState(false);
  const [status, setStatus] = useState<LoadStatus>("idle");
  const [error, setError] = useState<string | null>(null);
  const [detail, setDetail] = useState<QuickSessionDetail | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("idle");
    setError(null);
    setDetail(null);
  }, [activeSlug, reference.line, reference.sessionId]);

  useEffect(() => {
    if (!open || !activeSlug) return;
    let cancelled = false;
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setStatus("loading");
    setError(null);
    const query = reference.line
      ? { since: Math.max(0, reference.line - 6), limit: 11 }
      : { limit: 8 };
    void client
      .readQuickSession(activeSlug, reference.sessionId, query)
      .then((response) => {
        if (cancelled) return;
        if (!response.ok || !response.data) {
          setStatus("error");
          setError(response.error ?? "Failed to load Quick Session");
          return;
        }
        setDetail(response.data.session);
        setStatus("ok");
      })
      .catch((reason: unknown) => {
        if (cancelled) return;
        setStatus("error");
        setError(reason instanceof Error ? reason.message : "Failed to load Quick Session");
      });
    return () => {
      cancelled = true;
    };
  }, [activeSlug, open, reference.line, reference.sessionId]);

  const ref = formatQuickSessionRef(reference.sessionId, reference.line);
  const title = detail?.meta.title ?? "Quick Session";
  const messages = detail
    ? windowAround(detail.entries, reference.line)
    : [];

  return (
    <HoverCard open={open} onOpenChange={setOpen} openDelay={180} closeDelay={140}>
      <HoverCardTrigger asChild>
        <button
          type="button"
          draggable={Boolean(workspaceKey)}
          onDragStart={(event) => {
            if (!workspaceKey) {
              event.preventDefault();
              return;
            }
            event.dataTransfer.effectAllowed = "copy";
            event.dataTransfer.setData(
              QUICK_SESSION_DRAG_MIME,
              JSON.stringify({ ref, workspaceKey }),
            );
            event.dataTransfer.setData("text/plain", ref);
          }}
          className="inline-flex max-w-full items-center gap-1 rounded-md px-1 py-0.5 text-primary hover:bg-primary/10 hover:underline"
          onClick={(event) => {
            event.stopPropagation();
            setOpen((value) => !value);
          }}
          title={ref}
        >
          <MessageCircleMore className="size-3 shrink-0" />
          <span className="truncate">{detail?.meta.title ?? reference.sessionId}</span>
          {reference.line ? (
            <span className="font-mono text-[11px] text-primary/70">
              L{String(reference.line).padStart(6, "0")}
            </span>
          ) : null}
        </button>
      </HoverCardTrigger>
      <HoverCardContent align="start" className="w-[380px] max-w-[calc(100vw-24px)] p-0">
        <div className="max-h-[380px] overflow-y-auto p-3">
          <div className="flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="truncate text-sm font-semibold text-foreground">{title}</div>
              <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-text-muted">
                {detail ? <span>@{detail.meta.agent_id}</span> : null}
                <span className="font-mono">{reference.sessionId}</span>
              </div>
            </div>
            <Button
              variant="ghost"
              size="xs"
              className="gap-1"
              onClick={(event) => {
                event.stopPropagation();
                void navigator.clipboard.writeText(ref).then(() => {
                  setCopied(true);
                  setTimeout(() => setCopied(false), 1200);
                }).catch(() => setCopied(false));
              }}
            >
              {copied ? <Check className="size-3" /> : <Copy className="size-3" />}
              {copied ? "Copied" : "Copy"}
            </Button>
          </div>

          {detail ? (
            <>
              <div className="mt-2 flex flex-wrap items-center gap-1.5">
                <span className={cn("rounded-md border px-1.5 py-0.5 text-[11px]", quickSessionStatusClass(detail.meta.status))}>
                  {detail.meta.status}
                </span>
                {detail.archived ? (
                  <span className="rounded-md border border-border bg-muted/30 px-1.5 py-0.5 text-[11px] text-text-muted">
                    archived
                  </span>
                ) : null}
              </div>
              {detail.meta.summary ? (
                <p className="mt-2 text-xs leading-relaxed text-text-secondary">{detail.meta.summary}</p>
              ) : detail.meta.last_message_preview ? (
                <p className="mt-2 line-clamp-2 text-xs leading-relaxed text-text-secondary">
                  {detail.meta.last_message_preview}
                </p>
              ) : null}
            </>
          ) : null}

          <div className="mt-3 border-t border-border pt-3">
            {status === "loading" ? (
              <div className="flex items-center gap-2 py-3 text-xs text-text-muted">
                <Loader2 className="size-3.5 animate-spin" /> Loading preview…
              </div>
            ) : status === "error" ? (
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
