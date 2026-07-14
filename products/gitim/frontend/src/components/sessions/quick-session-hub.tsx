import { Check, MessageCircleMore, Plus } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Button } from "@/components/ui/button";
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover";
import { useAgentStore } from "@/hooks/use-agent-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useQuickSessionStore } from "@/hooks/use-quick-session-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import { workspaceIdentity } from "@/lib/workspace-key";
import { cn } from "@/lib/utils";
import { QuickSessionList } from "./quick-session-list";
import { QuickSessionPanel } from "./quick-session-panel";

const HOVER_CLOSE_DELAY_MS = 180;

export function QuickSessionHub() {
  const [open, setOpen] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [creating, setCreating] = useState(false);
  const [agentId, setAgentId] = useState("");
  const [firstMessage, setFirstMessage] = useState("");
  const [copied, setCopied] = useState(false);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const mode = useConnectionStore((state) => state.mode);
  const agents = useAgentStore((state) => state.agents);
  const activeSlug = useWorkspaceStore((state) => state.activeSlug);
  const workspaces = useWorkspaceStore((state) => state.workspaces);
  const activeWorkspace = workspaces.find((workspace) => workspace.slug === activeSlug);
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : "";

  const items = useQuickSessionStore((state) => state.items);
  const selectedId = useQuickSessionStore((state) => state.selectedId);
  const detail = useQuickSessionStore((state) =>
    state.selectedId ? state.detailById[state.selectedId] ?? null : null,
  );
  const runtime = useQuickSessionStore((state) =>
    state.selectedId ? state.runtimeById[state.selectedId] : undefined,
  );
  const showArchived = useQuickSessionStore((state) => state.showArchived);
  const loading = useQuickSessionStore((state) => state.loading);
  const errors = useQuickSessionStore((state) => state.errors);

  const clearCloseTimer = useCallback(() => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
    closeTimer.current = null;
  }, []);

  const enter = useCallback(() => {
    clearCloseTimer();
    if (activeSlug) setOpen(true);
  }, [activeSlug, clearCloseTimer]);

  const leave = useCallback(() => {
    clearCloseTimer();
    if (pinned) return;
    closeTimer.current = setTimeout(() => setOpen(false), HOVER_CLOSE_DELAY_MS);
  }, [clearCloseTimer, pinned]);

  useEffect(() => () => clearCloseTimer(), [clearCloseTimer]);
  useEffect(() => {
    if (!open || !activeSlug) return;
    void useQuickSessionStore.getState().refreshList(activeSlug);
  }, [activeSlug, open, showArchived]);
  const effectiveAgentId = agentId || agents[0]?.handler || "";

  async function copyRef(ref: string) {
    try {
      await navigator.clipboard.writeText(ref);
      setCopied(true);
      setTimeout(() => setCopied(false), 1200);
    } catch {
      setCopied(false);
    }
  }

  if (!activeSlug || !activeWorkspace) return null;

  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setPinned(false);
      }}
    >
      <div onPointerEnter={enter} onPointerLeave={leave} className="hidden md:block">
        <PopoverTrigger asChild>
          <button
            type="button"
            title="Quick Sessions"
            aria-label="Quick Sessions"
            aria-pressed={pinned}
            className={cn(
              "flex h-7 items-center gap-1.5 rounded-md px-2 text-xs text-text-muted transition-colors hover:bg-surface/60 hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50",
              open && "bg-primary/10 text-primary",
            )}
            onFocus={enter}
            onClick={(event) => {
              event.preventDefault();
              clearCloseTimer();
              if (pinned) {
                setPinned(false);
                setOpen(false);
              } else {
                setPinned(true);
                setOpen(true);
              }
            }}
          >
            <MessageCircleMore className="size-4" />
            <span>Sessions</span>
            {items.some((item) => item.status === "running") ? (
              <span className="size-1.5 rounded-full bg-primary" aria-label="Session running" />
            ) : null}
          </button>
        </PopoverTrigger>
      </div>
      <PopoverContent
        align="center"
        sideOffset={8}
        className="hidden h-[520px] w-[760px] max-w-[calc(100vw-24px)] overflow-hidden p-0 md:flex"
        onPointerEnter={enter}
        onPointerLeave={leave}
        onEscapeKeyDown={() => {
          setPinned(false);
          setOpen(false);
        }}
      >
        <section className="flex w-[280px] min-w-0 flex-col border-r border-border bg-card/80">
          <div className="flex items-center justify-between border-b border-border px-3 py-2.5">
            <div>
              <h2 className="text-sm font-semibold text-foreground">Quick Sessions</h2>
              <p className="text-[11px] text-text-muted">Focused, Git-synced conversations</p>
            </div>
            <Button
              variant="ghost"
              size="icon-xs"
              onClick={() => setCreating((value) => !value)}
              aria-label="New Quick Session"
            >
              <Plus className="size-4" />
            </Button>
          </div>

          {creating ? (
            <form
              className="space-y-2 border-b border-border p-3"
              onSubmit={(event) => {
                event.preventDefault();
                const message = firstMessage.trim();
                if (!effectiveAgentId || !message) return;
                void useQuickSessionStore
                  .getState()
                  .create(activeSlug, effectiveAgentId, message)
                  .then((id) => {
                    if (!id) return;
                    setFirstMessage("");
                    setCreating(false);
                  });
              }}
            >
              <select
                value={effectiveAgentId}
                onChange={(event) => setAgentId(event.target.value)}
                aria-label="Quick Session agent"
                className="h-8 w-full rounded-md border border-border bg-background px-2 text-xs focus:border-primary/60 focus:outline-none"
              >
                {agents.map((agent) => (
                  <option key={agent.id} value={agent.handler}>
                    {agent.name} (@{agent.handler})
                  </option>
                ))}
              </select>
              <textarea
                rows={3}
                value={firstMessage}
                onChange={(event) => setFirstMessage(event.target.value)}
                placeholder="What should this session focus on?"
                className="w-full resize-none rounded-md border border-border bg-background px-2.5 py-2 text-xs placeholder:text-text-muted focus:border-primary/60 focus:outline-none"
              />
              <Button type="submit" size="sm" className="w-full" disabled={!effectiveAgentId || !firstMessage.trim() || loading.create}>
                Start session
              </Button>
              {errors.create ? <p className="text-[11px] text-destructive">{errors.create}</p> : null}
            </form>
          ) : null}

          <label className="flex items-center gap-2 border-b border-border px-3 py-2 text-[11px] text-text-muted">
            <input
              type="checkbox"
              checked={showArchived}
              onChange={(event) => useQuickSessionStore.getState().setShowArchived(event.target.checked)}
              className="size-3.5 rounded border-border accent-primary"
            />
            Show archived
          </label>
          <QuickSessionList
            items={items}
            selectedId={selectedId}
            loading={loading.list}
            error={errors.list}
            workspaceKey={workspaceKey}
            onSelect={(id) => void useQuickSessionStore.getState().open(activeSlug, id)}
            onCopy={(ref) => void copyRef(ref)}
          />
        </section>

        <QuickSessionPanel
          detail={detail}
          runtime={runtime}
          loading={loading.detail}
          error={errors.detail ?? errors.send ?? errors.archive}
          sending={loading.send}
          onSend={(body) =>
            selectedId
              ? useQuickSessionStore.getState().send(activeSlug, selectedId, body)
              : Promise.resolve(false)
          }
          onArchive={async () => {
            if (!selectedId) return false;
            const ok = await useQuickSessionStore
              .getState()
              .archive(activeSlug, selectedId);
            if (ok) useQuickSessionStore.getState().select(null);
            return ok;
          }}
          onUnarchive={async () => {
            if (!selectedId) return false;
            const ok = await useQuickSessionStore
              .getState()
              .unarchive(activeSlug, selectedId);
            if (ok) useQuickSessionStore.getState().select(null);
            return ok;
          }}
          onCopy={(ref) => void copyRef(ref)}
        />
        {copied ? (
          <div className="absolute bottom-3 right-3 flex items-center gap-1 rounded-md border border-success/30 bg-success/10 px-2 py-1 text-[11px] text-success">
            <Check className="size-3" /> Copied
          </div>
        ) : null}
      </PopoverContent>
    </Popover>
  );
}
