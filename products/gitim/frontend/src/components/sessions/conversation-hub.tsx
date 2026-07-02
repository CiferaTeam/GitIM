import {
  Archive,
  GripVertical,
  MessageSquareText,
  PanelRightClose,
  Pin,
  Plus,
  Send,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useAgentStore } from "@/hooks/use-agent-store";
import {
  useQuickSessionStore,
  parseThread,
  shortSessionId,
  type ThreadMessage,
} from "@/hooks/use-quick-session-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import type { SessionRefDragPayload, QuickSessionStatus } from "@/lib/types";
import { SESSION_REF_MIME } from "@/lib/session-ref-dnd";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

function statusText(status: QuickSessionStatus): string {
  switch (status) {
    case "needs_title":
      return "needs title";
    case "running":
      return "running";
    case "error":
      return "error";
    case "archived":
      return "archived";
    case "active":
    default:
      return "active";
  }
}

function statusClass(status: QuickSessionStatus): string {
  switch (status) {
    case "needs_title":
      return "border-warning/30 bg-warning/10 text-warning";
    case "running":
      return "border-info/30 bg-info/10 text-info";
    case "error":
      return "border-destructive/30 bg-destructive/10 text-destructive";
    case "archived":
      return "border-border bg-muted/40 text-text-muted";
    case "active":
    default:
      return "border-success/30 bg-success/10 text-success";
  }
}

export function ConversationHub() {
  const agents = useAgentStore((s) => s.agents);
  const slug = useWorkspaceStore((s) => s.activeSlug);
  const agentIds = useMemo(() => agents.map((a) => a.id), [agents]);
  const agentOptions = useMemo(() => [...agentIds].sort(), [agentIds]);

  const store = useQuickSessionStore();
  const {
    sessions,
    loading,
    selectedId,
    select,
    refresh,
    create,
    loadDetail,
    sendMessage,
    setTitle,
    archive,
  } = store;

  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const [draft, setDraft] = useState("");
  const [newAgent, setNewAgent] = useState(agentOptions[0] ?? "");
  const [newMessage, setNewMessage] = useState("");
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const visibleSessions = sessions.filter((s) =>
    showArchived ? true : s.item.status !== "archived",
  );
  const activeCount = sessions.filter(
    (s) => s.item.status !== "archived" && s.item.status !== "error",
  ).length;

  const selectedSession = sessions.find((s) => s.item.id === selectedId) ?? visibleSessions[0] ?? null;

  // Track showArchived in a ref so the poll interval can access current value
  const showArchivedRef = useRef(showArchived);
  showArchivedRef.current = showArchived;

  // Poll for sessions when component mounts and slug is available
  useEffect(() => {
    if (!slug) return;
    refresh(slug, showArchivedRef.current);
    const interval = setInterval(() => {
      if (slug) refresh(slug, showArchivedRef.current);
    }, 8000);
    return () => clearInterval(interval);
  }, [slug, refresh]);

  // Load detail when selection changes
  useEffect(() => {
    if (slug && selectedSession && !selectedSession.detail && !selectedSession.detailLoading) {
      loadDetail(slug, selectedSession.item.id);
    }
  }, [slug, selectedSession, loadDetail]);

  function clearCloseTimer() {
    if (closeTimerRef.current) {
      clearTimeout(closeTimerRef.current);
      closeTimerRef.current = null;
    }
  }

  function handlePointerEnter() {
    if (pinned) return;
    clearCloseTimer();
    setOpen(true);
  }

  function handlePointerLeave() {
    if (pinned) return;
    clearCloseTimer();
    closeTimerRef.current = setTimeout(() => setOpen(false), 180);
  }

  const doCreate = useCallback(async () => {
    if (!slug || !newAgent || !newMessage.trim()) return;
    await create(slug, newAgent, newMessage.trim());
    setNewMessage("");
  }, [slug, newAgent, newMessage, create]);

  const doArchive = useCallback(async () => {
    if (!slug || !selectedSession) return;
    await archive(slug, selectedSession.item.id);
  }, [slug, selectedSession, archive]);

  const doSend = useCallback(async () => {
    if (!slug || !selectedSession || !draft.trim()) return;
    await sendMessage(slug, selectedSession.item.id, draft.trim());
    setDraft("");
  }, [slug, selectedSession, draft, sendMessage]);

  const panel = (
    <ConversationHubPanel
      sessions={visibleSessions}
      selectedSession={selectedSession}
      showArchived={showArchived}
      pinned={pinned}
      agentOptions={agentOptions}
      draft={draft}
      newAgent={newAgent}
      newMessage={newMessage}
      loading={loading}
      slug={slug}
      setTitle={setTitle}
      onSelect={select}
      onToggleArchived={() => {
        setShowArchived((prev) => {
          const next = !prev;
          if (slug) refresh(slug, next);
          return next;
        });
      }}
      onTogglePinned={() => {
        setPinned((v) => !v);
        setOpen(false);
      }}
      onClosePinned={() => setPinned(false)}
      onDraftChange={setDraft}
      onNewAgentChange={setNewAgent}
      onNewMessageChange={setNewMessage}
      onCreate={doCreate}
      onArchive={doArchive}
      onSend={doSend}
    />
  );

  const trigger = (
    <button
      type="button"
      className={cn(
        "inline-flex h-9 items-center gap-2 rounded-lg border border-border/70 bg-muted/50 px-3 text-sm font-medium text-text-secondary transition-colors hover:bg-surface/70 hover:text-foreground",
        (open || pinned) && "border-primary/40 bg-primary/10 text-primary",
      )}
      onClick={() => {
        if (pinned) {
          setPinned(false);
        } else {
          setOpen((v) => !v);
        }
      }}
    >
      <MessageSquareText className="size-4" />
      <span>对话</span>
      <span className="rounded-md bg-surface px-1.5 py-0.5 font-mono text-[11px] text-text-muted">
        {activeCount}
      </span>
    </button>
  );

  return (
    <div onPointerEnter={handlePointerEnter} onPointerLeave={handlePointerLeave}>
      <Popover open={!pinned && open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>{trigger}</PopoverTrigger>
        <PopoverContent
          align="end"
          sideOffset={8}
          className="w-[560px] max-w-[calc(100vw-32px)] p-0"
          onPointerEnter={handlePointerEnter}
          onPointerLeave={handlePointerLeave}
        >
          {panel}
        </PopoverContent>
      </Popover>

      {pinned && (
        <div className="fixed right-4 top-14 bottom-4 z-40 w-[560px] max-w-[calc(100vw-32px)] overflow-hidden rounded-xl border border-border bg-card shadow-2xl shadow-black/40">
          {panel}
        </div>
      )}
    </div>
  );
}

interface ConversationHubPanelProps {
  sessions: ReturnType<typeof useQuickSessionStore.getState>["sessions"];
  selectedSession: ReturnType<typeof useQuickSessionStore.getState>["sessions"][0] | null;
  showArchived: boolean;
  pinned: boolean;
  agentOptions: string[];
  draft: string;
  newAgent: string;
  newMessage: string;
  loading: boolean;
  slug: string | null;
  setTitle: ReturnType<typeof useQuickSessionStore.getState>["setTitle"];
  onSelect(id: string | null): void;
  onToggleArchived(): void;
  onTogglePinned(): void;
  onClosePinned(): void;
  onDraftChange(value: string): void;
  onNewAgentChange(value: string): void;
  onNewMessageChange(value: string): void;
  onCreate(): void;
  onArchive(): void;
  onSend(): void;
}

function ConversationHubPanel({
  sessions,
  selectedSession,
  showArchived,
  pinned,
  agentOptions,
  draft,
  newAgent,
  newMessage,
  loading,
  slug,
  setTitle,
  onSelect,
  onToggleArchived,
  onTogglePinned,
  onClosePinned,
  onDraftChange,
  onNewAgentChange,
  onNewMessageChange,
  onCreate,
  onArchive,
  onSend,
}: ConversationHubPanelProps) {
  const messages: ThreadMessage[] = useMemo(() => {
    if (!selectedSession?.detail?.thread) return [];
    return parseThread(selectedSession.detail.thread);
  }, [selectedSession?.detail?.thread]);

  const meta = selectedSession?.item;
  const detailLoading = selectedSession?.detailLoading;
  const [editingTitle, setEditingTitle] = useState(false);
  const [draftTitle, setDraftTitle] = useState("");

  return (
    <section className="grid h-[560px] max-h-[calc(100vh-80px)] grid-cols-[220px_minmax(0,1fr)] overflow-hidden bg-card text-foreground">
      {/* Left: Session list */}
      <aside className="flex min-w-0 flex-col border-r border-border bg-background/35">
        <div className="flex items-center justify-between border-b border-border px-3 py-2">
          <div className="min-w-0">
            <div className="text-sm font-semibold">短对话</div>
            <div className="text-[11px] text-text-muted">聚合所有 agent 的独立小任务</div>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="icon-xs"
            title={pinned ? "Unpin" : "Pin"}
            onClick={onTogglePinned}
          >
            {pinned ? <PanelRightClose className="size-3.5" /> : <Pin className="size-3.5" />}
          </Button>
        </div>

        {/* New session form */}
        <div className="border-b border-border p-2">
          <div className="grid grid-cols-[1fr_auto] gap-1.5">
            <textarea
              value={newMessage}
              onChange={(e) => onNewMessageChange(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter" && !e.shiftKey) {
                  e.preventDefault();
                  onCreate();
                }
              }}
              placeholder="第一句话..."
              rows={2}
              className="min-w-0 resize-none rounded-md border border-border bg-surface px-2 py-1.5 text-xs leading-relaxed outline-none focus:border-primary/60"
            />
            <select
              value={newAgent}
              onChange={(e) => onNewAgentChange(e.target.value)}
              className="w-[96px] rounded-md border border-border bg-surface px-1.5 py-1.5 text-xs outline-none focus:border-primary/60"
            >
              {agentOptions.map((agent) => (
                <option key={agent} value={agent}>
                  @{agent}
                </option>
              ))}
            </select>
          </div>
          <Button
            type="button"
            size="xs"
            className="mt-1.5 w-full"
            disabled={!newMessage.trim() || !newAgent || loading}
            onClick={onCreate}
          >
            <Plus className="size-3" />
            发起
          </Button>
        </div>

        <div className="flex items-center justify-between px-3 py-2 text-[11px] text-text-muted">
          <span>{showArchived ? "All sessions" : "Active sessions"}</span>
          <button
            type="button"
            onClick={onToggleArchived}
            className="rounded-md px-1.5 py-0.5 hover:bg-surface hover:text-foreground"
          >
            {showArchived ? "Hide archived" : "Show archived"}
          </button>
        </div>

        <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2">
          {sessions.map((s) => (
            <SessionListItem
              key={s.item.id}
              item={s.item}
              selected={selectedSession?.item.id === s.item.id}
              onSelect={() => onSelect(s.item.id)}
            />
          ))}
          {sessions.length === 0 && !loading && (
            <div className="rounded-lg border border-dashed border-border p-3 text-xs text-text-muted">
              No sessions yet.
            </div>
          )}
          {loading && sessions.length === 0 && (
            <div className="text-xs text-text-muted p-3">Loading...</div>
          )}
        </div>
      </aside>

      {/* Right: Detail / Messages */}
      <main className="flex min-w-0 flex-col">
        {meta ? (
          <>
            <div className="flex items-start justify-between gap-3 border-b border-border px-3 py-2">
              <div className="min-w-0">
                <div className="flex min-w-0 items-center gap-2">
                  {editingTitle ? (
                    <input
                      className="h-7 min-w-0 rounded border border-primary bg-card px-1.5 text-sm font-semibold outline-none"
                      value={draftTitle}
                      onChange={(e) => setDraftTitle(e.target.value)}
                      onKeyDown={async (e) => {
                        if (e.key === "Enter") {
                          e.preventDefault();
                          const trimmed = draftTitle.trim();
                          if (trimmed && slug && meta?.id) {
                            await setTitle(slug, meta.id, trimmed);
                            setEditingTitle(false);
                          }
                        } else if (e.key === "Escape") {
                          setEditingTitle(false);
                        }
                      }}
                      onBlur={async () => {
                        const trimmed = draftTitle.trim();
                        if (trimmed && slug && meta?.id) {
                          await setTitle(slug, meta.id, trimmed);
                        }
                        setEditingTitle(false);
                      }}
                      autoFocus
                    />
                  ) : (
                    <h2
                      className={cn(
                        "truncate text-sm font-semibold",
                        meta.status !== "archived" &&
                          "cursor-pointer hover:text-primary",
                      )}
                      onClick={() => {
                        if (meta.status !== "archived") {
                          setDraftTitle(meta.title || "");
                          setEditingTitle(true);
                        }
                      }}
                      title={
                        meta.status !== "archived"
                          ? "Click to edit title"
                          : undefined
                      }
                    >
                      {meta.title || "Untitled"}
                    </h2>
                  )}
                  <span className="shrink-0 rounded-md border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[11px] text-text-muted">
                    {meta.ref_}
                  </span>
                </div>
                <div className="mt-1 flex items-center gap-1.5 text-[11px] text-text-muted">
                  <span>@{meta.agent_id}</span>
                  <span>·</span>
                  <span>{shortSessionId(meta.id)}</span>
                  <span>·</span>
                  <span className={cn("rounded-md border px-1 py-0", statusClass(meta.status))}>
                    {statusText(meta.status)}
                  </span>
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  disabled={meta.status === "archived"}
                  onClick={onArchive}
                >
                  <Archive className="size-3" />
                  Archive
                </Button>
                {pinned && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon-xs"
                    onClick={onClosePinned}
                  >
                    <X className="size-3.5" />
                  </Button>
                )}
              </div>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
              {detailLoading ? (
                <div className="text-xs text-text-muted">Loading...</div>
              ) : (
                <div className="space-y-2">
                  {messages.map((msg, i) => (
                    <div
                      key={i}
                      className={cn(
                        "flex",
                        msg.author !== meta.agent_id ? "justify-end" : "justify-start",
                      )}
                    >
                      <div
                        className={cn(
                          "max-w-[78%] rounded-lg border px-3 py-2 text-sm leading-relaxed",
                          msg.author !== meta.agent_id
                            ? "border-primary/25 bg-primary/15 text-foreground"
                            : "border-border bg-surface/70 text-text-secondary",
                        )}
                      >
                        <div className="mb-1 flex items-center gap-1.5 text-[11px] text-text-muted">
                          <span>
                            {msg.author !== meta.agent_id ? "you" : `@${msg.author}`}
                          </span>
                        </div>
                        <div className="whitespace-pre-wrap break-words">{msg.body}</div>
                      </div>
                    </div>
                  ))}
                  {messages.length === 0 && (
                    <div className="text-xs text-text-muted">No messages yet.</div>
                  )}
                </div>
              )}
            </div>

            <div className="border-t border-border p-3">
              <div className="flex gap-2">
                <textarea
                  value={draft}
                  onChange={(e) => onDraftChange(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      onSend();
                    }
                  }}
                  disabled={meta.status === "archived"}
                  placeholder={meta.status === "archived" ? "Archived" : "Continue..."}
                  rows={2}
                  className="min-w-0 flex-1 resize-none rounded-lg border border-border bg-surface px-3 py-2 text-sm leading-relaxed outline-none focus:border-primary/60 disabled:opacity-60"
                />
                <Button
                  type="button"
                  size="icon-sm"
                  className="self-end"
                  disabled={!draft.trim() || meta.status === "archived" || detailLoading}
                  onClick={onSend}
                >
                  <Send className="size-4" />
                </Button>
              </div>
            </div>
          </>
        ) : (
          <div className="flex h-full items-center justify-center text-sm text-text-muted">
            Select or create a short conversation.
          </div>
        )}
      </main>
    </section>
  );
}

function SessionListItem({
  item,
  selected,
  onSelect,
}: {
  item: ReturnType<typeof useQuickSessionStore.getState>["sessions"][0]["item"];
  selected: boolean;
  onSelect(): void;
}) {
  const payload: SessionRefDragPayload = {
    id: item.id,
    title: item.title || "Untitled",
    agent: item.agent_id,
    ref: item.ref_,
  };

  return (
    <button
      type="button"
      draggable={item.status !== "archived"}
      onDragStart={(e) => {
        e.dataTransfer.effectAllowed = "copy";
        e.dataTransfer.setData(SESSION_REF_MIME, JSON.stringify(payload));
        e.dataTransfer.setData("text/plain", `${payload.ref} ${payload.title}`);
      }}
      onClick={onSelect}
      className={cn(
        "mb-1.5 flex w-full items-start gap-2 rounded-lg border px-2 py-2 text-left transition-colors",
        selected
          ? "border-primary/40 bg-primary/10"
          : "border-transparent hover:border-border hover:bg-surface/60",
        item.status === "archived" && "opacity-60",
      )}
    >
      <GripVertical className="mt-0.5 size-3.5 shrink-0 text-text-faint" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-xs font-semibold text-foreground">
          {item.title || "Untitled"}
        </div>
        <div className="mt-1 flex items-center gap-1.5 text-[11px] text-text-muted">
          <span>@{item.agent_id}</span>
          <span>·</span>
          {item.last_message_preview && (
            <>
              <span className="truncate">{item.last_message_preview}</span>
              <span>·</span>
            </>
          )}
          <span className={cn("rounded-md border px-1.5 py-0.5 text-[10px]", statusClass(item.status))}>
            {statusText(item.status)}
          </span>
        </div>
      </div>
    </button>
  );
}
