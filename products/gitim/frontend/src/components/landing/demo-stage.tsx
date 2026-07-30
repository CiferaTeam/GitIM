import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  ArrowLeft,
  Bot,
  FileCode,
  FolderOpen,
  GitCommit,
  Hash,
  MessageSquare,
  Pause,
  Play,
  RotateCcw,
  SkipBack,
  User,
  Volume2,
  VolumeX,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { buildFileTree, type TreeNode } from "@/lib/demo-story/tree";
import narrationManifest from "@/lib/demo-story/narration-manifest.json";
import {
  incidentScenario,
  useDemoPlayer,
  type DemoCard,
  type DemoEffect,
  type DemoFrame,
  type DemoMember,
  type DemoMessage,
  type DemoScenario,
  type DemoState,
  type DemoView,
} from "@/lib/demo-story";

/** frameId → narration audio duration (ms), from the offline TTS manifest. */
const FRAME_DURATIONS: Record<string, number> = Object.fromEntries(
  Object.entries(narrationManifest).map(([id, v]) => [id, v.durationMs]),
);

/**
 * Landing demo stage v2 — single scene, no tabs.
 * Left: chat main view (channel stream or one card's discussion thread).
 * Right: persistent MEMBERS / CARDS / GIT sections.
 * Causal highlights (arrow / pulse / badge) fan out after every message;
 * no global dimming — everything stays fully lit.
 */

export function DemoStage({
  autoPlay = false,
  fullHeight = false,
  onClose,
}: {
  autoPlay?: boolean;
  /** Stretch to fill the viewport-height container the landing page gives us. */
  fullHeight?: boolean;
  onClose?: () => void;
}) {
  // ?debug=1 forces step-through mode (Next-step button) even without
  // prefers-reduced-motion — used for manual QA and screenshot passes.
  const debugStep = useMemo(
    () =>
      typeof window !== "undefined" &&
      new URLSearchParams(window.location.search).has("debug"),
    [],
  );
  const player = useDemoPlayer(incidentScenario, {
    autoplay: autoPlay,
    stepMode: debugStep,
    frameDurations: FRAME_DURATIONS,
  });
  const [muted, setMuted] = useState(false);
  const stageRef = useRef<HTMLDivElement>(null);
  const frame = player.currentFrame;
  const view: DemoView = frame?.view ?? { kind: "channel" };
  const typedValue = useTyping(
    frame,
    player.status === "playing",
    player.reducedMotion,
  );
  useNarrationAudio(frame, player.status, muted);

  return (
    <div
      ref={stageRef}
      className={cn(
        "relative w-full rounded-2xl border border-border bg-card/90 shadow-xl overflow-hidden flex flex-col text-left",
        fullHeight ? "lg:h-[calc(100vh-4.5rem)]" : "lg:h-[min(78vh,44rem)]",
      )}
      data-testid="demo-stage"
    >
      <ChapterBar
        scenario={incidentScenario}
        frameIndex={player.frameIndex}
        onJump={player.goTo}
        onClose={onClose}
      />
      <NarrationBar frame={frame} />

      <div className="flex-1 min-h-0 grid grid-cols-1 lg:grid-cols-[1fr_18rem] lg:grid-rows-[minmax(0,1fr)] gap-3 p-3">
        <MainView
          view={view}
          state={player.state}
          frame={frame}
          revealFresh={player.status === "playing" && !player.reducedMotion}
          animated={!player.reducedMotion}
          inputValue={typedValue ?? ""}
        />
        <SideBar state={player.state} />
      </div>

      <ControlsBar
        player={player}
        total={incidentScenario.frames.length}
        muted={muted}
        onToggleMute={() => setMuted((m) => !m)}
      />
      <EffectsOverlay frame={frame} stageRef={stageRef} />
    </div>
  );
}

// ---------------------------------------------------------------------------
// Chapter progress bar
// ---------------------------------------------------------------------------

function ChapterBar({
  scenario,
  frameIndex,
  onJump,
  onClose,
}: {
  scenario: DemoScenario;
  frameIndex: number;
  onJump: (index: number) => void;
  onClose?: () => void;
}) {
  const ranges = useMemo(() => {
    const result: { id: string; label: string; start: number; end: number; count: number }[] = [];
    scenario.chapters.forEach((ch, i) => {
      const count = scenario.frames.filter((f) => f.chapter === ch.id).length;
      const start = i === 0 ? 0 : result[i - 1].end + 1;
      result.push({ ...ch, start, end: start + count - 1, count });
    });
    return result;
  }, [scenario]);

  return (
    <div
      className="h-11 border-b border-border flex items-center gap-2 sm:gap-4 px-3 bg-surface/40 shrink-0"
      data-testid="demo-chapter-bar"
    >
      {ranges.map((r) => {
        const progress =
          frameIndex < r.start
            ? 0
            : frameIndex > r.end
              ? 1
              : (frameIndex - r.start + 1) / r.count;
        const current = frameIndex >= r.start && frameIndex <= r.end;
        return (
          <button
            key={r.id}
            type="button"
            onClick={() => onJump(r.start)}
            className="flex items-center gap-2 group"
            title={`Jump to ${r.label}`}
            data-testid={`demo-chapter-${r.id}`}
          >
            <span
              className={cn(
                "text-xs font-medium whitespace-nowrap transition-colors",
                current
                  ? "text-foreground"
                  : "text-text-muted group-hover:text-text-secondary",
              )}
            >
              {r.label}
            </span>
            <span className="w-8 sm:w-14 h-1 rounded-full bg-surface overflow-hidden">
              <span
                className="block h-full rounded-full bg-primary transition-[width] duration-300"
                style={{ width: `${progress * 100}%` }}
              />
            </span>
          </button>
        );
      })}
      <span
        className="ml-auto text-xs text-text-muted tabular-nums shrink-0"
        data-testid="demo-frame-counter"
      >
        <span className="text-foreground font-medium">
          {Math.max(0, frameIndex + 1)}
        </span>
        {" / "}
        {scenario.frames.length}
      </span>
      {onClose && (
        <button
          type="button"
          onClick={onClose}
          className="flex h-7 shrink-0 items-center gap-1.5 rounded-md border border-primary/30 bg-primary/10 px-2.5 text-xs font-semibold text-primary transition-colors hover:border-primary/50 hover:bg-primary/20"
          aria-label="Back to overview"
          data-testid="demo-close"
        >
          <ArrowLeft className="size-3.5" />
          <span>Back to overview</span>
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Narration bar (doubles as future captions)
// ---------------------------------------------------------------------------

function NarrationBar({ frame }: { frame: DemoFrame | null }) {
  return (
    <div
      className="h-10 border-b border-border flex items-center gap-2.5 px-3 shrink-0"
      data-testid="demo-narration"
    >
      <span className="size-1.5 rounded-full bg-primary shrink-0" />
      <span
        className="text-sm font-medium text-foreground shrink-0"
        data-testid="demo-narration-title"
      >
        {frame?.title ?? "Deterministic replay"}
      </span>
      <span
        className="text-xs text-text-muted truncate"
        data-testid="demo-narration-caption"
      >
        {frame?.caption ??
          "Press play — no AI runs here; every frame is precomputed."}
      </span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Main view: channel stream or one card's discussion thread
// ---------------------------------------------------------------------------

function MainView({
  view,
  state,
  frame,
  revealFresh,
  animated,
  inputValue,
}: {
  view: DemoView;
  state: DemoState;
  frame: DemoFrame | null;
  revealFresh: boolean;
  /** False in step-through mode: view switches stay instantaneous there. */
  animated: boolean;
  inputValue: string;
}) {
  const transition = useViewTransition(view, animated);

  return (
    <section
      className="relative rounded-xl border border-border bg-surface/30 overflow-hidden min-h-[20rem] lg:min-h-0 flex flex-col"
      data-testid="demo-main-panel"
    >
      {/* The current view always comes first in DOM order so anchor lookups
          and testids resolve to it, never to the transient layer. */}
      {transition?.dir === "in" ? (
        <ViewPanel
          view={view}
          state={state}
          frame={frame}
          revealFresh={revealFresh}
          inputValue={inputValue}
          className="absolute inset-0 z-10 bg-surface demo-view-enter"
        />
      ) : (
        <ViewPanel
          view={view}
          state={state}
          frame={frame}
          revealFresh={revealFresh}
          inputValue={inputValue}
          className="flex-1 min-h-0"
        />
      )}
      {transition?.dir === "in" && (
        <ViewPanel
          view={transition.prev}
          state={state}
          frame={null}
          revealFresh={false}
          inputValue=""
          transient
          className="flex-1 min-h-0"
        />
      )}
      {transition?.dir === "out" && (
        <ViewPanel
          view={transition.prev}
          state={state}
          frame={null}
          revealFresh={false}
          inputValue=""
          transient
          className="absolute inset-0 z-10 bg-surface demo-view-exit"
        />
      )}
    </section>
  );
}

function viewTransitionKey(view: DemoView): string {
  return view.kind === "card" ? `card:${view.cardId}` : "channel";
}

/**
 * Tracks the previous view for one 300ms slide/fade hand-off whenever the
 * main view changes. Entering a card slides the card layer in from the
 * right; going back slides the card layer out to the right.
 */
function useViewTransition(
  view: DemoView,
  animated: boolean,
): { prev: DemoView; dir: "in" | "out" } | null {
  const [st, setSt] = useState<{
    key: string;
    view: DemoView;
    trans: { prev: DemoView; dir: "in" | "out" } | null;
  }>(() => ({ key: viewTransitionKey(view), view, trans: null }));

  const key = viewTransitionKey(view);
  if (key !== st.key) {
    setSt({
      key,
      view,
      trans:
        animated && st.key !== ""
          ? { prev: st.view, dir: key.startsWith("card") ? "in" : "out" }
          : null,
    });
  }

  const trans = st.trans;
  useEffect(() => {
    if (!trans) return;
    const handle = window.setTimeout(() => {
      setSt((s) => (s.trans === trans ? { ...s, trans: null } : s));
    }, 320);
    return () => window.clearTimeout(handle);
  }, [trans]);

  return st.trans;
}

function ViewPanel({
  view,
  state,
  frame,
  revealFresh,
  inputValue,
  transient = false,
  className,
}: {
  view: DemoView;
  state: DemoState;
  frame: DemoFrame | null;
  revealFresh: boolean;
  inputValue: string;
  /** Transient layers are the leaving/background copy during a transition. */
  transient?: boolean;
  className?: string;
}) {
  const cardId = view.kind === "card" ? view.cardId : null;
  const card = cardId
    ? state.cards.find((c) => c.cardId === cardId)
    : undefined;
  const showCard = view.kind === "card" && card !== undefined;
  const messages = showCard
    ? (state.messages.cards[card.cardId] ?? [])
    : state.messages.channel;

  // Line numbers introduced by the current frame — those agent messages get
  // the typing-dots → typewriter reveal instead of popping in instantly.
  const freshLines = useMemo(() => {
    const set = new Set<number>();
    for (const c of frame?.uiChanges ?? []) {
      if (!showCard && c.type === "channel-message") set.add(c.message.lineNumber);
      if (showCard && c.type === "card-message" && c.cardId === card?.cardId) {
        set.add(c.message.lineNumber);
      }
    }
    return set;
  }, [frame, showCard, card?.cardId]);

  const testIdSuffix = transient ? "-transient" : "";

  const scrollRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (transient) return; // the leaving layer stays frozen mid-transition
    const el = scrollRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [messages.length, view.kind, cardId, transient]);

  return (
    <div className={cn("flex flex-col", className)}>
      {showCard ? (
        <div className="h-9 px-3 flex items-center gap-2 border-b border-border shrink-0 text-xs min-w-0">
          <span
            className="flex items-center gap-1 text-text-muted shrink-0"
            data-testid={`demo-card-back${testIdSuffix}`}
          >
            <ArrowLeft className="size-3" />
            back to #release-v2-4
          </span>
          <span className="text-text-faint shrink-0">/</span>
          <span className="font-mono text-text-secondary shrink-0">
            {card.cardId}
          </span>
          <span className="font-medium text-foreground truncate">
            {card.title}
          </span>
          <CardStatusBadge status={card.status} />
        </div>
      ) : (
        <div className="h-9 px-3 flex items-center gap-2 border-b border-border shrink-0">
          <Hash className="size-3.5 text-text-muted shrink-0" />
          <span className="text-sm font-medium text-foreground">
            release-v2-4
          </span>
          <span className="text-[10px] leading-none text-text-faint font-mono ml-auto">
            release log
          </span>
        </div>
      )}

      <div
        ref={scrollRef}
        className="flex-1 overflow-auto p-3 space-y-3 min-h-0"
        data-testid={`${showCard ? "demo-card-panel" : "demo-chat-panel"}${testIdSuffix}`}
      >
        {messages.length === 0 && (
          <p className="text-xs text-text-muted italic">
            No messages yet — the work starts here.
          </p>
        )}
        {messages.map((msg) => (
          <MessageBubble
            key={`${showCard ? card.cardId : "ch"}-${msg.lineNumber}`}
            msg={msg}
            anchor={
              showCard
                ? (msg.anchor ?? `card-msg-${msg.lineNumber}`)
                : `chat-msg-${msg.lineNumber}`
            }
            members={state.members}
            reveal={revealFresh && freshLines.has(msg.lineNumber)}
          />
        ))}
      </div>

      <div className="px-3 pb-3 shrink-0">
        <div
          className="flex items-center gap-2 px-3 py-2 rounded-lg border border-border bg-surface"
          data-anchor={transient ? undefined : "chat-input"}
        >
          <MessageSquare className="size-4 text-text-muted shrink-0" />
          <input
            type="text"
            readOnly
            value={inputValue}
            placeholder={
              showCard ? `Reply in ${card.cardId}…` : "Message #release-v2-4…"
            }
            className="flex-1 min-w-0 bg-transparent text-sm text-text-secondary outline-none placeholder:text-text-faint"
            data-testid={`demo-chat-input${testIdSuffix}`}
          />
          <button
            type="button"
            tabIndex={-1}
            className="px-3 py-1 text-xs font-medium bg-primary text-white rounded-md shrink-0"
          >
            Send
          </button>
        </div>
      </div>
    </div>
  );
}

function MessageBubble({
  msg,
  anchor,
  members,
  reveal = false,
}: {
  msg: DemoMessage;
  anchor: string;
  members: DemoMember[];
  reveal?: boolean;
}) {
  const member = members.find((m) => m.handler === msg.author);
  const isHuman = member?.kind === "human" || msg.author === "lewis";
  // Agents don't type into the human's input box — their messages surface as
  // a typing indicator, then stream in character by character.
  const revealActive = reveal && !isHuman;
  const shown = useRevealText(msg.body, revealActive);
  return (
    <div
      className="w-fit max-w-[92%] rounded-lg"
      data-anchor={anchor}
      data-testid={`demo-message-${anchor}`}
    >
      <div
        className={cn(
          "rounded-lg px-3 py-2 text-xs leading-relaxed border",
          isHuman
            ? "bg-primary/10 border-primary/20 text-foreground"
            : "bg-surface border-border text-foreground",
        )}
      >
        <div className="flex items-center gap-1.5 mb-1">
          {isHuman ? (
            <User className="size-3 text-primary shrink-0" />
          ) : (
            <Bot className="size-3 text-primary shrink-0" />
          )}
          <span className="font-medium text-text-secondary">
            {member?.displayName ?? msg.author}
          </span>
          <span
            className="text-[10px] leading-none text-text-faint font-mono ml-auto pl-3"
            title={msg.timestamp}
          >
            {formatDemoTime(msg.timestamp)}
          </span>
        </div>
        {shown === null ? (
          <div
            className="flex items-center gap-1 py-1"
            data-testid={`demo-typing-${anchor}`}
          >
            <span className="demo-typing-dot" />
            <span className="demo-typing-dot" />
            <span className="demo-typing-dot" />
          </div>
        ) : (
          <p className="whitespace-pre-wrap">{renderBody(shown)}</p>
        )}
        {shown === msg.body &&
          msg.commandChips?.map((chip, i) => (
            <div
              key={i}
              className="mt-1.5 flex items-center gap-1.5 rounded-md border border-border bg-background/60 px-2 py-1 font-mono text-[10px] leading-tight text-text-secondary"
              title={chip}
              data-anchor={`${anchor}-chip-${i + 1}`}
              data-testid={`demo-chip-${anchor}-${i + 1}`}
            >
              <span className="text-primary shrink-0">$</span>
              <span className="truncate">{chip}</span>
            </div>
          ))}
      </div>
    </div>
  );
}

/**
 * Agent message reveal: null while the typing indicator shows, then the body
 * streams in at a fixed cadence. Inactive → full text immediately.
 */
function useRevealText(text: string, active: boolean): string | null {
  const [phase, setPhase] = useState<{ text: string; len: number }>({
    text: "",
    len: -1, // -1 = typing-dots phase
  });

  // Reset synchronously when a different message becomes the reveal target.
  if (phase.text !== text || (!active && phase.len !== text.length)) {
    setPhase({ text, len: active ? -1 : text.length });
  }

  useEffect(() => {
    if (!active) return;
    if (phase.len === -1) {
      const handle = window.setTimeout(() => {
        setPhase((s) => (s.text === text ? { ...s, len: 0 } : s));
      }, 650);
      return () => window.clearTimeout(handle);
    }
    if (phase.len >= text.length) return;
    const handle = window.setTimeout(() => {
      setPhase((s) => ({ ...s, len: Math.min(s.len + 2, text.length) }));
    }, 1000 / 45);
    return () => window.clearTimeout(handle);
  }, [active, phase.len, text]);

  if (phase.len < 0) return null;
  return text.slice(0, phase.len);
}

function renderBody(body: string) {
  const parts = body.split(/(<@[a-z0-9-]+>|<#[^>]+>)/g);
  return parts.map((part, i) =>
    /^<[@#]/.test(part) ? (
      <span key={i} className="text-primary font-medium">
        {part}
      </span>
    ) : (
      <span key={i}>{part}</span>
    ),
  );
}

function formatDemoTime(ts: string): string {
  const m = /T(\d{2})(\d{2})\d{2}Z$/.exec(ts);
  return m ? `${m[1]}:${m[2]}` : ts;
}

// ---------------------------------------------------------------------------
// Right sidebar: MEMBERS / CARDS / GIT
// ---------------------------------------------------------------------------

function SideBar({ state }: { state: DemoState }) {
  return (
    <aside
      className="flex flex-col gap-3 min-h-0"
      data-testid="demo-sidebar"
    >
      <MembersSection members={state.members} />
      <CardsSection cards={state.cards} />
      <GitSection state={state} />
    </aside>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div className="text-[10px] leading-none font-medium text-text-muted mb-2 uppercase tracking-wide">
      {children}
    </div>
  );
}

function MembersSection({ members }: { members: DemoMember[] }) {
  return (
    <section
      className="rounded-xl border border-border bg-surface/30 p-3 shrink-0"
      data-testid="demo-members"
    >
      <SectionLabel>Members</SectionLabel>
      <div className="space-y-0.5">
        {members.map((m) => (
          <div
            key={m.handler}
            className="flex items-center gap-2 px-2 py-1.5 rounded-md"
            title={`@${m.handler}${m.provider ? ` · ${m.provider}` : ""}`}
            data-anchor={`member-${m.handler}`}
            data-testid={`demo-member-${m.handler}`}
          >
            {m.kind === "human" ? (
              <User className="size-3.5 text-text-secondary shrink-0" />
            ) : (
              <Bot className="size-3.5 text-primary shrink-0" />
            )}
            <span className="text-xs font-medium text-foreground truncate">
              {m.displayName}
            </span>
            {m.provider && (
              <span className="text-[10px] leading-none text-text-faint font-mono truncate">
                {m.provider}
              </span>
            )}
            <span
              className={cn(
                "ml-auto text-[9px] leading-none px-1.5 py-0.5 rounded-full uppercase font-medium shrink-0",
                m.status === "working"
                  ? "bg-primary/15 text-primary"
                  : "bg-surface text-text-muted border border-border",
              )}
            >
              {m.status}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}

function CardsSection({ cards }: { cards: DemoCard[] }) {
  return (
    <section
      className="rounded-xl border border-border bg-surface/30 p-3 shrink-0"
      data-testid="demo-cards"
    >
      <SectionLabel>Cards</SectionLabel>
      {cards.length === 0 ? (
        <p className="text-xs text-text-muted italic">No cards yet.</p>
      ) : (
        <div className="space-y-1">
          {cards.map((c) => (
            <div
              key={c.cardId}
              className="rounded-md border border-border bg-surface/40 px-2 py-1"
              data-anchor={`card-${c.cardId}`}
              data-testid={`demo-card-${c.cardId}`}
            >
              <div className="flex items-center gap-1.5">
                <span className="text-[10px] leading-tight font-mono text-text-muted shrink-0">
                  {c.cardId}
                </span>
                <span className="text-[10px] leading-tight text-text-faint font-mono truncate">
                  → @{c.assignee}
                </span>
                <CardStatusBadge status={c.status} />
              </div>
              <div className="text-xs font-medium text-foreground truncate">
                {c.title}
              </div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}

function CardStatusBadge({ status }: { status: DemoCard["status"] }) {
  return (
    <span
      className={cn(
        "ml-auto text-[9px] leading-none px-1.5 py-0.5 rounded-full uppercase font-medium shrink-0",
        status === "doing" && "bg-primary/15 text-primary",
        status === "done" && "bg-success/10 text-success",
        status === "todo" && "bg-surface text-text-muted border border-border",
      )}
    >
      {status}
    </span>
  );
}

function GitSection({ state }: { state: DemoState }) {
  const paths = useMemo(() => Object.keys(state.files).sort(), [state.files]);
  const tree = useMemo(() => buildFileTree(paths), [paths]);
  const latest = state.commits[state.commits.length - 1];

  return (
    <section
      className="rounded-xl border border-border bg-surface/30 p-3 shrink-0 lg:shrink lg:flex-1 lg:min-h-0 flex flex-col"
      data-testid="demo-git"
    >
      <div className="flex items-center justify-between mb-2 shrink-0">
        <SectionLabel>Git</SectionLabel>
        <span className="text-[10px] leading-none text-text-faint font-mono -mt-2">
          {state.commits.length} commits
        </span>
      </div>
      <div
        className="font-mono text-[11px] leading-relaxed lg:flex-1 lg:min-h-0 lg:overflow-y-auto"
        data-testid="demo-git-tree"
      >
        <GitTreeNode node={tree} files={state.files} />
      </div>
      {latest && (
        <div
          className="mt-2 flex items-center gap-1.5 rounded-md border border-border bg-surface/50 px-2 py-1.5 shrink-0"
          data-anchor="git:latest-commit"
          data-testid="demo-latest-commit"
          title={latest.message}
        >
          <GitCommit className="size-3 text-primary shrink-0" />
          <span className="font-mono text-[10px] leading-none text-text-secondary shrink-0">
            {latest.id}
          </span>
          <span className="text-[10px] leading-none text-text-muted truncate">
            {latest.message}
          </span>
        </div>
      )}
    </section>
  );
}

function GitTreeNode({
  node,
  files,
}: {
  node: TreeNode;
  files: DemoState["files"];
}) {
  if (node.kind === "file") {
    const file = files[node.path];
    return (
      <div
        className="flex items-center gap-1 px-1 py-0.5 rounded"
        title={node.path}
        data-anchor={`git:${node.path}`}
        data-testid={`demo-git-${node.path}`}
      >
        <StatusMarker status={file?.status} />
        <FileCode className="size-3 shrink-0 text-text-muted" />
        <span
          className={cn(
            "truncate",
            file?.status === "added" && "text-primary",
            file?.status === "modified" && "text-foreground",
            file?.status === "unchanged" && "text-text-secondary",
          )}
        >
          {node.name}
        </span>
      </div>
    );
  }

  return (
    <div className={node.depth >= 0 ? "ml-1.5 pl-1.5 border-l border-border/40" : ""}>
      {node.depth >= 0 && (
        <div
          className="flex items-center gap-1 px-1 py-0.5 text-text-muted"
          title={node.path}
          data-anchor={`git:${node.path}`}
          data-testid={`demo-git-${node.path}`}
        >
          <FolderOpen className="size-3 shrink-0" />
          <span className="truncate">{node.name}/</span>
        </div>
      )}
      {node.children.map((child) => (
        <GitTreeNode key={child.path} node={child} files={files} />
      ))}
    </div>
  );
}

function StatusMarker({
  status,
}: {
  status: DemoState["files"][string]["status"] | undefined;
}) {
  if (status === "added") return <span className="text-primary w-3">+</span>;
  if (status === "modified") return <span className="text-primary w-3">~</span>;
  return <span className="text-text-faint w-3"> </span>;
}

// ---------------------------------------------------------------------------
// Controls bar
// ---------------------------------------------------------------------------

function ControlsBar({
  player,
  total,
  muted,
  onToggleMute,
}: {
  player: ReturnType<typeof useDemoPlayer>;
  total: number;
  muted: boolean;
  onToggleMute: () => void;
}) {
  const playing = player.status === "playing";
  const finished = player.status === "finished";

  function handleReplay() {
    player.reset();
    if (!player.reducedMotion) {
      queueMicrotask(() => player.play());
    }
  }

  return (
    <div className="h-14 border-t border-border bg-surface/40 px-3 flex items-center gap-2 shrink-0">
      <button
        type="button"
        onClick={player.prev}
        disabled={player.isFirstFrame}
        className="p-2 rounded-lg border border-border text-text-muted hover:text-foreground hover:bg-surface disabled:opacity-30 transition-colors"
        aria-label="Previous step"
        data-testid="demo-prev"
      >
        <SkipBack className="size-4" />
      </button>

      {player.reducedMotion ? (
        <button
          type="button"
          onClick={player.next}
          disabled={player.isLastFrame && player.status === "paused"}
          className="px-4 py-2 rounded-lg bg-primary text-primary-foreground text-sm font-medium hover:bg-primary/90 disabled:opacity-50"
          data-testid="demo-next"
        >
          Next step
        </button>
      ) : (
        <button
          type="button"
          onClick={finished ? handleReplay : playing ? player.pause : player.play}
          className={cn(
            "flex items-center gap-2 px-4 py-2 rounded-lg text-sm font-medium transition-colors",
            playing
              ? "border border-border text-foreground hover:bg-surface"
              : "bg-primary text-primary-foreground hover:bg-primary/90",
          )}
          data-testid="demo-play-pause"
        >
          {finished ? (
            <>
              <RotateCcw className="size-4" />
              Replay
            </>
          ) : playing ? (
            <>
              <Pause className="size-4" />
              Pause
            </>
          ) : (
            <>
              <Play className="size-4" />
              Play demo
            </>
          )}
        </button>
      )}

      {!player.reducedMotion && (
        <button
          type="button"
          onClick={handleReplay}
          className="p-2 rounded-lg border border-border text-text-muted hover:text-foreground hover:bg-surface transition-colors"
          aria-label="Replay"
          data-testid="demo-replay"
        >
          <RotateCcw className="size-4" />
        </button>
      )}

      <div className="ml-auto flex items-center gap-3">
        <button
          type="button"
          onClick={onToggleMute}
          title={muted ? "Unmute narration" : "Mute narration"}
          aria-label={muted ? "Unmute narration" : "Mute narration"}
          className="p-2 rounded-lg border border-border text-text-muted hover:text-foreground hover:bg-surface transition-colors"
          data-testid="demo-mute"
        >
          {muted ? (
            <VolumeX className="size-4" />
          ) : (
            <Volume2 className="size-4" />
          )}
        </button>
        <span className="text-sm text-text-muted tabular-nums">
          <span className="font-medium text-foreground">
            {Math.max(0, player.frameIndex + 1)}
          </span>
          {" / "}
          {total}
        </span>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Typing animation (chat input typewriter)
// ---------------------------------------------------------------------------

function useTyping(
  frame: DemoFrame | null,
  playing: boolean,
  reducedMotion: boolean,
): string | null {
  const typing = frame?.typing ?? null;
  const frameId = frame?.id ?? null;
  const cps = typing?.cps ?? 50;
  const [ts, setTs] = useState<{ frameId: string | null; len: number }>({
    frameId: null,
    len: 0,
  });

  // Reset synchronously when the frame changes (setState-during-render is the
  // documented "adjust state when props change" pattern — avoids a flash of
  // stale text when stepping).
  if (ts.frameId !== frameId) {
    setTs({
      frameId,
      len: typing && playing && !reducedMotion ? 0 : (typing?.text.length ?? 0),
    });
  }

  useEffect(() => {
    if (!typing || !playing || reducedMotion) return;
    if (ts.len >= typing.text.length) return;
    const handle = window.setTimeout(() => {
      setTs((s) => ({ ...s, len: Math.min(s.len + 1, typing.text.length) }));
    }, 1000 / cps);
    return () => window.clearTimeout(handle);
  }, [ts.len, typing, playing, reducedMotion, cps]);

  if (!typing) return null;
  return typing.text.slice(0, ts.len);
}

// ---------------------------------------------------------------------------
// Narration audio (pre-generated MiMo TTS wav files, see narration-manifest)
// ---------------------------------------------------------------------------

/**
 * Plays the current frame's narration wav while the demo is playing and
 * unmuted. Pausing/muting suspends the clip; switching frames stops it.
 * Autoplay-policy rejections are swallowed — the visuals carry on silently.
 */
function useNarrationAudio(
  frame: DemoFrame | null,
  status: ReturnType<typeof useDemoPlayer>["status"],
  muted: boolean,
) {
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const frameId = frame?.id ?? null;

  useEffect(() => {
    const current = audioRef.current;
    if (current && current.dataset.frameId !== frameId) {
      current.pause();
      current.currentTime = 0;
      audioRef.current = null;
    }

    const entry = frameId
      ? (narrationManifest as Record<string, { file: string }>)[frameId]
      : undefined;
    if (status !== "playing" || muted || !frameId || !entry) {
      audioRef.current?.pause();
      return;
    }

    if (!audioRef.current) {
      const audio = new Audio(`/${entry.file}`);
      audio.dataset.frameId = frameId;
      audioRef.current = audio;
    }
    const playPromise = audioRef.current.play();
    // jsdom returns undefined; browsers may reject on autoplay policy.
    void playPromise?.catch(() => {});
  }, [frameId, status, muted]);

  // Stop on unmount.
  useEffect(
    () => () => {
      audioRef.current?.pause();
      audioRef.current = null;
    },
    [],
  );
}

// ---------------------------------------------------------------------------
// Causal highlight overlay (arrow / pulse / badge)
// ---------------------------------------------------------------------------

interface AnchorRect {
  x: number;
  y: number;
  width: number;
  height: number;
}

type ArrowDir = "left" | "right" | "up" | "down";

/**
 * Scroll an anchor element into view within its nearest scrollable ancestor
 * that lives inside the stage. Deliberately avoids window.scrollIntoView so
 * the landing page itself never jumps while the demo plays.
 */
function scrollAnchorIntoView(stage: HTMLElement, id: string) {
  const el = stage.querySelector<HTMLElement>(`[data-anchor="${id}"]`);
  if (!el) return;
  let container: HTMLElement | null = el.parentElement;
  while (container && container !== stage) {
    const overflowY = window.getComputedStyle(container).overflowY;
    if (
      (overflowY === "auto" || overflowY === "scroll") &&
      container.scrollHeight > container.clientHeight
    ) {
      break;
    }
    container = container.parentElement;
  }
  if (!container || container === stage) return;

  const elRect = el.getBoundingClientRect();
  const cRect = container.getBoundingClientRect();
  const margin = 8;
  if (elRect.top < cRect.top + margin) {
    container.scrollTop -= cRect.top + margin - elRect.top;
  } else if (elRect.bottom > cRect.bottom - margin) {
    container.scrollTop += elRect.bottom - (cRect.bottom - margin);
  }
}

function arrowGeometry(from: AnchorRect, to: AnchorRect) {
  const fx = from.x + from.width / 2;
  const fy = from.y + from.height / 2;
  const tx = to.x + to.width / 2;
  const ty = to.y + to.height / 2;
  const dx = tx - fx;
  const dy = ty - fy;

  let sx: number, sy: number, ex: number, ey: number, dir: ArrowDir;
  if (Math.abs(dx) >= Math.abs(dy)) {
    if (dx >= 0) {
      sx = from.x + from.width;
      sy = fy;
      ex = to.x;
      ey = ty;
      dir = "right";
    } else {
      sx = from.x;
      sy = fy;
      ex = to.x + to.width;
      ey = ty;
      dir = "left";
    }
  } else if (dy >= 0) {
    sx = fx;
    sy = from.y + from.height;
    ex = tx;
    ey = to.y;
    dir = "down";
  } else {
    sx = fx;
    sy = from.y;
    ex = tx;
    ey = to.y + to.height;
    dir = "up";
  }

  // Small gap so the arrowhead does not overlap the target border.
  const gap = 4;
  if (dir === "right") ex -= gap;
  else if (dir === "left") ex += gap;
  else if (dir === "down") ey -= gap;
  else ey += gap;

  const span = dir === "left" || dir === "right" ? Math.abs(ex - sx) : Math.abs(ey - sy);
  const bend = Math.max(20, Math.min(56, span / 2));
  let c1x: number, c1y: number, c2x: number, c2y: number;
  if (dir === "right") {
    c1x = sx + bend;
    c1y = sy;
    c2x = ex - bend;
    c2y = ey;
  } else if (dir === "left") {
    c1x = sx - bend;
    c1y = sy;
    c2x = ex + bend;
    c2y = ey;
  } else if (dir === "down") {
    c1x = sx;
    c1y = sy + bend;
    c2x = ex;
    c2y = ey - bend;
  } else {
    c1x = sx;
    c1y = sy - bend;
    c2x = ex;
    c2y = ey + bend;
  }

  const d = `M ${sx} ${sy} C ${c1x} ${c1y}, ${c2x} ${c2y}, ${ex} ${ey}`;
  const t = 0.5;
  const u = 1 - t;
  const mid = {
    x: u * u * u * sx + 3 * u * u * t * c1x + 3 * u * t * t * c2x + t * t * t * ex,
    y: u * u * u * sy + 3 * u * u * t * c1y + 3 * u * t * t * c2y + t * t * t * ey,
  };
  return { d, dir, end: { x: ex, y: ey }, mid };
}

function arrowheadPoints(dir: ArrowDir, end: { x: number; y: number }): string {
  const { x, y } = end;
  switch (dir) {
    case "right":
      return `${x + 2},${y} ${x - 5},${y - 4} ${x - 5},${y + 4}`;
    case "left":
      return `${x - 2},${y} ${x + 5},${y - 4} ${x + 5},${y + 4}`;
    case "down":
      return `${x},${y + 2} ${x - 4},${y - 5} ${x + 4},${y - 5}`;
    case "up":
      return `${x},${y - 2} ${x - 4},${y + 5} ${x + 4},${y + 5}`;
  }
}

function EffectsOverlay({
  frame,
  stageRef,
}: {
  frame: DemoFrame | null;
  stageRef: React.RefObject<HTMLDivElement | null>;
}) {
  const [rects, setRects] = useState<Record<string, AnchorRect>>({});
  const [size, setSize] = useState({ w: 0, h: 0 });

  useLayoutEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    // Bring off-screen targets into view inside their own scroll containers
    // (chat stream, git tree) before measuring — otherwise arrows point into
    // the void below the fold.
    const ids = new Set<string>();
    for (const e of frame?.effects ?? []) {
      if (e.kind === "arrow") {
        ids.add(e.from);
        ids.add(e.to);
      } else {
        ids.add(e.target);
      }
    }
    for (const id of ids) scrollAnchorIntoView(stage, id);

    const compute = () => {
      const stageBox = stage.getBoundingClientRect();
      setSize({ w: stageBox.width, h: stageBox.height });
      const next: Record<string, AnchorRect> = {};
      for (const e of frame?.effects ?? []) {
        const ids = e.kind === "arrow" ? [e.from, e.to] : [e.target];
        for (const id of ids) {
          if (next[id]) continue;
          const el = stage.querySelector(`[data-anchor="${id}"]`);
          if (!el) continue;
          const r = el.getBoundingClientRect();
          next[id] = {
            x: r.left - stageBox.left,
            y: r.top - stageBox.top,
            width: r.width,
            height: r.height,
          };
        }
      }
      setRects(next);
    };
    compute();
    const raf = requestAnimationFrame(compute);
    let ro: ResizeObserver | null = null;
    if (typeof ResizeObserver !== "undefined") {
      ro = new ResizeObserver(compute);
      ro.observe(stage);
      // Follow reveal-driven growth on the targets themselves: agent
      // messages stream in character by character, so a rect measured at
      // frame start is stale within a second.
      for (const id of ids) {
        const el = stage.querySelector(`[data-anchor="${id}"]`);
        if (el) ro.observe(el);
      }
    }
    return () => {
      cancelAnimationFrame(raf);
      ro?.disconnect();
    };
  }, [frame, stageRef]);

  if (!frame?.effects || frame.effects.length === 0) return null;

  const arrows = frame.effects.filter(
    (e): e is Extract<DemoEffect, { kind: "arrow" }> => e.kind === "arrow",
  );
  const pulses = frame.effects.filter(
    (e): e is Extract<DemoEffect, { kind: "pulse" }> => e.kind === "pulse",
  );
  const badges = frame.effects.filter(
    (e): e is Extract<DemoEffect, { kind: "badge" }> => e.kind === "badge",
  );
  const badgeStack = new Map<string, number>();

  return (
    <div
      className="absolute inset-0 z-20 pointer-events-none overflow-hidden"
      aria-hidden="true"
      data-testid="demo-effects-overlay"
    >
      <svg
        className="absolute inset-0"
        width={size.w}
        height={size.h}
        viewBox={`0 0 ${size.w} ${size.h}`}
      >
        {arrows.map((e, i) => {
          const from = rects[e.from];
          const to = rects[e.to];
          if (!from || !to) return null;
          const g = arrowGeometry(from, to);
          return (
            <g key={`${frame.id}-a-${i}`}>
              <path className="demo-arrow" d={g.d} />
              <polygon
                className="demo-arrowhead"
                points={arrowheadPoints(g.dir, g.end)}
              />
            </g>
          );
        })}
      </svg>

      {arrows.map((e, i) => {
        if (!e.label) return null;
        const from = rects[e.from];
        const to = rects[e.to];
        if (!from || !to) return null;
        const g = arrowGeometry(from, to);
        return (
          <div
            key={`${frame.id}-al-${i}`}
            className="demo-fx-label absolute rounded-full border border-primary/40 bg-card px-2 py-0.5 text-[10px] leading-none font-medium text-primary shadow-md whitespace-nowrap -translate-x-1/2 -translate-y-1/2"
            style={{ left: g.mid.x, top: g.mid.y }}
          >
            {e.label}
          </div>
        );
      })}

      {pulses.map((e, i) => {
        const r = rects[e.target];
        if (!r) return null;
        return (
          <div
            key={`${frame.id}-p-${i}`}
            className="demo-pulse-ring absolute rounded-lg"
            style={{
              left: r.x - 3,
              top: r.y - 3,
              width: r.width + 6,
              height: r.height + 6,
            }}
          />
        );
      })}

      {badges.map((e, i) => {
        const r = rects[e.target];
        if (!r) return null;
        const stack = badgeStack.get(e.target) ?? 0;
        badgeStack.set(e.target, stack + 1);
        return (
          <div
            key={`${frame.id}-b-${i}`}
            className="demo-badge absolute rounded-full border border-primary/40 bg-card px-2 py-0.5 text-[10px] font-medium leading-none text-primary shadow-md whitespace-nowrap"
            style={{
              left: r.x + r.width - 6,
              // Hug the target's top line: vertically centered for single-line
              // rows (git tree, members, commit row), top-aligned for taller
              // cards so the badge never covers the card title text.
              top: r.y + Math.min(r.height / 2, 12) - stack * 20,
            }}
          >
            {e.text}
          </div>
        );
      })}
    </div>
  );
}
