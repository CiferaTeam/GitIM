import {
  Archive,
  ArrowUpRight,
  GripVertical,
  MessageSquareText,
  PanelRightClose,
  Pin,
  Plus,
  Send,
  X,
} from "lucide-react";
import { useMemo, useRef, useState } from "react";
import { useAgentStore } from "@/hooks/use-agent-store";
import { SESSION_DEMO_MIME, type SessionDemoDragPayload } from "@/lib/session-demo-dnd";
import { cn } from "@/lib/utils";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";

type DemoSessionStatus = "active" | "waiting" | "archived";

interface DemoMessage {
  id: string;
  author: "human" | "agent";
  body: string;
  time: string;
}

interface DemoSession {
  id: string;
  title: string;
  agent: string;
  status: DemoSessionStatus;
  updated: string;
  ref: string;
  messages: DemoMessage[];
}

const SEED_SESSIONS: DemoSession[] = [
  {
    id: "qs-031",
    title: "梳理 X 账号并制定规划",
    agent: "hermes",
    status: "active",
    updated: "2m",
    ref: "session:qs-031",
    messages: [
      {
        id: "m1",
        author: "human",
        body: "先看账号定位，给我一个 30 天内容计划的切法。",
        time: "10:18",
      },
      {
        id: "m2",
        author: "agent",
        body: "我会先拆受众、内容柱和可复用栏目，再给一个可执行的周节奏。",
        time: "10:19",
      },
    ],
  },
  {
    id: "qs-044",
    title: "研究 Codex 电脑使用转发方案",
    agent: "codex",
    status: "waiting",
    updated: "8m",
    ref: "session:qs-044",
    messages: [
      {
        id: "m1",
        author: "human",
        body: "这个先别进正式频道，帮我独立判断下风险。",
        time: "10:03",
      },
      {
        id: "m2",
        author: "agent",
        body: "我会把权限边界、浏览器状态依赖、失败恢复三个点拆开看。",
        time: "10:04",
      },
    ],
  },
  {
    id: "qs-052",
    title: "设计短对话卡片",
    agent: "atlas",
    status: "active",
    updated: "now",
    ref: "session:qs-052",
    messages: [
      {
        id: "m1",
        author: "human",
        body: "我想要顶部聚合入口，hover 后可以直接聊。",
        time: "10:27",
      },
      {
        id: "m2",
        author: "agent",
        body: "这应该是高一层的 Conversation Hub，每条短对话内部仍绑定单 agent。",
        time: "10:28",
      },
    ],
  },
];

function shortId(id: string): string {
  return id.replace(/^qs-/, "#");
}

function statusText(status: DemoSessionStatus): string {
  switch (status) {
    case "waiting":
      return "waiting";
    case "archived":
      return "archived";
    case "active":
    default:
      return "active";
  }
}

function statusClass(status: DemoSessionStatus): string {
  switch (status) {
    case "waiting":
      return "border-warning/30 bg-warning/10 text-warning";
    case "archived":
      return "border-border bg-muted/40 text-text-muted";
    case "active":
    default:
      return "border-success/30 bg-success/10 text-success";
  }
}

function fallbackAgentOptions(): string[] {
  return ["hermes", "codex", "atlas"];
}

function titleFromMessage(message: string): string {
  const normalized = message.replace(/\s+/g, " ").trim();
  if (!normalized) return "新短对话";
  return normalized.length > 18 ? `${normalized.slice(0, 18)}...` : normalized;
}

export function ConversationHubDemo() {
  const agents = useAgentStore((s) => s.agents);
  const agentIds = useMemo(() => agents.map((agent) => agent.id), [agents]);
  const agentOptions = useMemo(() => {
    const merged = new Set([...agentIds, ...fallbackAgentOptions()]);
    return [...merged].sort();
  }, [agentIds]);

  const [sessions, setSessions] = useState<DemoSession[]>(SEED_SESSIONS);
  const [selectedId, setSelectedId] = useState("qs-052");
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [showArchived, setShowArchived] = useState(false);
  const [draft, setDraft] = useState("");
  const [newAgent, setNewAgent] = useState(agentOptions[0] ?? "hermes");
  const [newMessage, setNewMessage] = useState("");
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const visibleSessions = sessions.filter((session) =>
    showArchived ? true : session.status !== "archived",
  );
  const activeCount = sessions.filter((session) => session.status !== "archived").length;
  const selectedSession =
    sessions.find((session) => session.id === selectedId) ?? visibleSessions[0] ?? null;

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

  function createSession() {
    const body = newMessage.trim();
    if (!body) return;
    const id = `qs-${Date.now().toString(36).slice(-5)}`;
    const session: DemoSession = {
      id,
      title: titleFromMessage(body),
      agent: newAgent,
      status: "waiting",
      updated: "now",
      ref: `session:${id}`,
      messages: [
        {
          id: `${id}-human`,
          author: "human",
          body,
          time: "now",
        },
      ],
    };
    setSessions((prev) => [session, ...prev]);
    setSelectedId(id);
    setNewMessage("");
  }

  function archiveSelected() {
    if (!selectedSession) return;
    setSessions((prev) =>
      prev.map((session) =>
        session.id === selectedSession.id
          ? { ...session, status: "archived", updated: "now" }
          : session,
      ),
    );
    const next = sessions.find(
      (session) => session.id !== selectedSession.id && session.status !== "archived",
    );
    if (next) {
      setSelectedId(next.id);
    }
  }

  function sendMessage() {
    const body = draft.trim();
    if (!body || !selectedSession) return;
    const humanMessage: DemoMessage = {
      id: `${selectedSession.id}-${Date.now()}`,
      author: "human",
      body,
      time: "now",
    };
    setSessions((prev) =>
      prev.map((session) =>
        session.id === selectedSession.id
          ? {
              ...session,
              status: "waiting",
              updated: "now",
              messages: [...session.messages, humanMessage],
            }
          : session,
      ),
    );
    setDraft("");

    const selectedAtSend = selectedSession.id;
    window.setTimeout(() => {
      const agentMessage: DemoMessage = {
        id: `${selectedAtSend}-agent-${Date.now()}`,
        author: "agent",
        body: "收到。我会先在这条短对话里收敛，如果需要多人协作再升级到 channel。",
        time: "now",
      };
      setSessions((prev) =>
        prev.map((session) =>
          session.id === selectedAtSend
            ? {
                ...session,
                status: "active",
                updated: "now",
                messages: [...session.messages, agentMessage],
              }
            : session,
        ),
      );
    }, 480);
  }

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
      onSelect={setSelectedId}
      onToggleArchived={() => setShowArchived((value) => !value)}
      onTogglePinned={() => {
        setPinned((value) => !value);
        setOpen(false);
      }}
      onClosePinned={() => setPinned(false)}
      onDraftChange={setDraft}
      onNewAgentChange={setNewAgent}
      onNewMessageChange={setNewMessage}
      onCreate={createSession}
      onArchive={archiveSelected}
      onSend={sendMessage}
    />
  );

  return (
    <div onPointerEnter={handlePointerEnter} onPointerLeave={handlePointerLeave}>
      <Popover open={!pinned && open} onOpenChange={setOpen}>
        <PopoverTrigger asChild>
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
                setOpen((value) => !value);
              }
            }}
          >
            <MessageSquareText className="size-4" />
            <span>对话</span>
            <span className="rounded-md bg-surface px-1.5 py-0.5 font-mono text-[11px] text-text-muted">
              {activeCount}
            </span>
          </button>
        </PopoverTrigger>
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
  sessions: DemoSession[];
  selectedSession: DemoSession | null;
  showArchived: boolean;
  pinned: boolean;
  agentOptions: string[];
  draft: string;
  newAgent: string;
  newMessage: string;
  onSelect(id: string): void;
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
  return (
    <section className="grid h-[560px] max-h-[calc(100vh-80px)] grid-cols-[220px_minmax(0,1fr)] overflow-hidden bg-card text-foreground">
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
            title={pinned ? "Unpin conversation hub" : "Pin conversation hub"}
            onClick={onTogglePinned}
          >
            {pinned ? <PanelRightClose className="size-3.5" /> : <Pin className="size-3.5" />}
          </Button>
        </div>

        <div className="border-b border-border p-2">
          <div className="grid grid-cols-[1fr_auto] gap-1.5">
            <textarea
              value={newMessage}
              onChange={(event) => onNewMessageChange(event.target.value)}
              placeholder="第一句话..."
              rows={2}
              className="min-w-0 resize-none rounded-md border border-border bg-surface px-2 py-1.5 text-xs leading-relaxed outline-none focus:border-primary/60"
            />
            <select
              value={newAgent}
              onChange={(event) => onNewAgentChange(event.target.value)}
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
            disabled={!newMessage.trim()}
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
          {sessions.map((session) => (
            <SessionListItem
              key={session.id}
              session={session}
              selected={selectedSession?.id === session.id}
              onSelect={() => onSelect(session.id)}
            />
          ))}
          {sessions.length === 0 && (
            <div className="rounded-lg border border-dashed border-border p-3 text-xs text-text-muted">
              No sessions.
            </div>
          )}
        </div>
      </aside>

      <main className="flex min-w-0 flex-col">
        {selectedSession ? (
          <>
            <div className="flex items-start justify-between gap-3 border-b border-border px-3 py-2">
              <div className="min-w-0">
                <div className="flex min-w-0 items-center gap-2">
                  <h2 className="truncate text-sm font-semibold">{selectedSession.title}</h2>
                  <span className="shrink-0 rounded-md border border-border bg-muted/40 px-1.5 py-0.5 font-mono text-[11px] text-text-muted">
                    {selectedSession.ref}
                  </span>
                </div>
                <div className="mt-1 flex items-center gap-1.5 text-[11px] text-text-muted">
                  <span>@{selectedSession.agent}</span>
                  <span>·</span>
                  <span>{shortId(selectedSession.id)}</span>
                  <span>·</span>
                  <span>{selectedSession.updated}</span>
                </div>
              </div>
              <div className="flex shrink-0 items-center gap-1">
                <Button
                  type="button"
                  variant="ghost"
                  size="xs"
                  title="Archive session"
                  disabled={selectedSession.status === "archived"}
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
                    title="Close pinned panel"
                    onClick={onClosePinned}
                  >
                    <X className="size-3.5" />
                  </Button>
                )}
              </div>
            </div>

            <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
              <div className="space-y-2">
                {selectedSession.messages.map((message) => (
                  <div
                    key={message.id}
                    className={cn(
                      "flex",
                      message.author === "human" ? "justify-end" : "justify-start",
                    )}
                  >
                    <div
                      className={cn(
                        "max-w-[78%] rounded-lg border px-3 py-2 text-sm leading-relaxed",
                        message.author === "human"
                          ? "border-primary/25 bg-primary/15 text-foreground"
                          : "border-border bg-surface/70 text-text-secondary",
                      )}
                    >
                      <div className="mb-1 flex items-center gap-1.5 text-[11px] text-text-muted">
                        <span>{message.author === "human" ? "you" : `@${selectedSession.agent}`}</span>
                        <span>{message.time}</span>
                      </div>
                      <div className="whitespace-pre-wrap break-words">{message.body}</div>
                    </div>
                  </div>
                ))}
              </div>
            </div>

            <div className="border-t border-border p-3">
              <div className="flex gap-2">
                <textarea
                  value={draft}
                  onChange={(event) => onDraftChange(event.target.value)}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" && !event.shiftKey) {
                      event.preventDefault();
                      onSend();
                    }
                  }}
                  disabled={selectedSession.status === "archived"}
                  placeholder={
                    selectedSession.status === "archived"
                      ? "已归档"
                      : "继续这条短对话..."
                  }
                  rows={2}
                  className="min-w-0 flex-1 resize-none rounded-lg border border-border bg-surface px-3 py-2 text-sm leading-relaxed outline-none focus:border-primary/60 disabled:opacity-60"
                />
                <Button
                  type="button"
                  size="icon-sm"
                  className="self-end"
                  disabled={!draft.trim() || selectedSession.status === "archived"}
                  onClick={onSend}
                  title="Send"
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
  session,
  selected,
  onSelect,
}: {
  session: DemoSession;
  selected: boolean;
  onSelect(): void;
}) {
  const payload: SessionDemoDragPayload = {
    id: session.id,
    title: session.title,
    agent: session.agent,
    ref: session.ref,
  };

  return (
    <button
      type="button"
      draggable={session.status !== "archived"}
      onDragStart={(event) => {
        event.dataTransfer.effectAllowed = "copy";
        event.dataTransfer.setData(SESSION_DEMO_MIME, JSON.stringify(payload));
        event.dataTransfer.setData("text/plain", `${payload.ref} ${payload.title}`);
      }}
      onClick={onSelect}
      className={cn(
        "mb-1.5 flex w-full items-start gap-2 rounded-lg border px-2 py-2 text-left transition-colors",
        selected
          ? "border-primary/40 bg-primary/10"
          : "border-transparent hover:border-border hover:bg-surface/60",
        session.status === "archived" && "opacity-60",
      )}
    >
      <GripVertical className="mt-0.5 size-3.5 shrink-0 text-text-faint" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-xs font-semibold text-foreground">{session.title}</div>
        <div className="mt-1 flex items-center gap-1.5 text-[11px] text-text-muted">
          <span>@{session.agent}</span>
          <span>·</span>
          <span>{session.updated}</span>
        </div>
        <div className="mt-1.5 flex items-center gap-1.5">
          <span className={cn("rounded-md border px-1.5 py-0.5 text-[10px]", statusClass(session.status))}>
            {statusText(session.status)}
          </span>
          <span className="inline-flex items-center gap-1 rounded-md border border-border bg-muted/30 px-1.5 py-0.5 font-mono text-[10px] text-text-muted">
            {shortId(session.id)}
            <ArrowUpRight className="size-2.5" />
          </span>
        </div>
      </div>
    </button>
  );
}
