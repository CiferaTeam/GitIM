import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { RefreshCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { useBoardStore } from "@/hooks/use-board-store";
import { useConnectionStore } from "@/hooks/use-connection-store";
import { useTimezoneStore } from "@/hooks/use-timezone";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import { formatDateTime } from "@/lib/timezone";
import type { BoardReadResponse, BoardSummary } from "@/lib/types";
import { writeUiState } from "@/lib/ui-state";
import { workspaceIdentity } from "@/lib/workspace-key";
import { cn } from "@/lib/utils";

type LoadState = "idle" | "loading" | "error";

interface MarkdownBlock {
  heading: string | null;
  body: string;
}

export function BoardsView() {
  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  const workspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;
  const boards = useBoardStore((s) => s.boards);
  const selectedHandler = useBoardStore((s) => s.selectedHandler);
  const selectedBoard = useBoardStore((s) => s.selectedBoard);
  const setBoards = useBoardStore((s) => s.setBoards);
  const setSelectedHandler = useBoardStore((s) => s.setSelectedHandler);
  const setSelectedBoard = useBoardStore((s) => s.setSelectedBoard);
  const [listState, setListState] = useState<LoadState>("idle");
  const [detailState, setDetailState] = useState<LoadState>("idle");
  const [error, setError] = useState<string | null>(null);
  const activeSlugRef = useRef(activeSlug);
  const listRequestIdRef = useRef(0);

  useEffect(() => {
    activeSlugRef.current = activeSlug;
  }, [activeSlug]);

  useEffect(() => {
    return () => {
      listRequestIdRef.current += 1;
    };
  }, []);

  const refreshBoards = useCallback(async (options?: { reloadSelectedDetail?: boolean }) => {
    const requestSlug = activeSlug;
    if (!requestSlug) return;
    if (activeSlugRef.current !== requestSlug) return;
    const requestId = listRequestIdRef.current + 1;
    listRequestIdRef.current = requestId;
    const previousSelected = options?.reloadSelectedDetail
      ? useBoardStore.getState().selectedHandler
      : null;
    setListState("loading");
    setError(null);
    const res = await client.listBoards(requestSlug);
    if (
      listRequestIdRef.current !== requestId ||
      activeSlugRef.current !== requestSlug
    ) {
      return;
    }
    if (!res.ok || !res.data) {
      setListState("error");
      setError(res.error ?? "Failed to load boards");
      return;
    }
    setBoards(res.data.boards);
    setListState("idle");
    if (
      !previousSelected ||
      !res.data.boards.some((board) => board.handler === previousSelected)
    ) {
      return;
    }

    setDetailState("loading");
    const detailRes = await client.showBoard(requestSlug, previousSelected);
    if (
      listRequestIdRef.current !== requestId ||
      activeSlugRef.current !== requestSlug ||
      useBoardStore.getState().selectedHandler !== previousSelected
    ) {
      return;
    }
    if (!detailRes.ok || !detailRes.data) {
      setDetailState("error");
      setError(detailRes.error ?? "Failed to load board");
      return;
    }
    setSelectedBoard(detailRes.data);
    setDetailState("idle");
  }, [activeSlug, setBoards, setSelectedBoard]);

  useEffect(() => {
    let cancelled = false;
    queueMicrotask(() => {
      if (!cancelled) void refreshBoards();
    });
    return () => {
      cancelled = true;
    };
  }, [refreshBoards]);

  useEffect(() => {
    const requestSlug = activeSlug;
    const requestHandler = selectedHandler;
    if (!requestSlug || !requestHandler) {
      setSelectedBoard(null);
      return;
    }
    const slug: string = requestSlug;
    const handler: string = requestHandler;
    let cancelled = false;
    void loadBoard();
    async function loadBoard() {
      await Promise.resolve();
      if (cancelled) return;
      setDetailState("loading");
      setError(null);
      const res = await client.showBoard(slug, handler);
      if (cancelled) return;
      if (!res.ok || !res.data) {
        setDetailState("error");
        setError(res.error ?? "Failed to load board");
        return;
      }
      setSelectedBoard(res.data);
      setError(null);
      setDetailState("idle");
    }
    return () => {
      cancelled = true;
    };
  }, [activeSlug, selectedHandler, setSelectedBoard]);

  const activeSummary = useMemo(
    () => boards.find((board) => board.handler === selectedHandler) ?? null,
    [boards, selectedHandler],
  );

  return (
    <div className="flex h-full min-w-0 flex-col overflow-hidden">
      <div className="flex items-center justify-between gap-3 border-b border-border px-4 py-3">
        <div className="min-w-0">
          <h1 className="truncate text-xl font-semibold">Boards</h1>
          <p className="truncate text-xs text-muted-foreground">
            Public showboards for the current workspace
          </p>
        </div>
        <Button
          type="button"
          variant="outline"
          size="icon-sm"
          aria-label="Refresh boards"
          title="Refresh boards"
          onClick={() => void refreshBoards({ reloadSelectedDetail: true })}
        >
          <RefreshCcw className={cn("size-4", listState === "loading" && "animate-spin")} />
        </Button>
      </div>

      {error && (
        <div className="border-b border-destructive/30 bg-destructive/10 px-4 py-2 text-sm text-destructive">
          {error}
        </div>
      )}

      {boards.length === 0 && listState !== "loading" ? (
        <EmptyBoards />
      ) : (
        <div className="grid min-h-0 min-w-0 flex-1 grid-rows-[auto_1fr] overflow-hidden md:grid-cols-[18rem_1fr] md:grid-rows-1">
          <BoardList
            boards={boards}
            selectedHandler={selectedHandler}
            onSelect={(handler) => {
              setSelectedHandler(handler);
              if (workspaceKey) writeUiState(workspaceKey, { boardHandler: handler });
            }}
          />
          <BoardDetail
            board={selectedBoard}
            summary={activeSummary}
            loading={detailState === "loading"}
          />
        </div>
      )}
    </div>
  );
}

function BoardList({
  boards,
  selectedHandler,
  onSelect,
}: {
  boards: BoardSummary[];
  selectedHandler: string | null;
  onSelect: (handler: string) => void;
}) {
  const timezone = useTimezoneStore((s) => s.timezone);
  return (
    <aside className="min-w-0 overflow-hidden border-b border-border md:border-b-0 md:border-r">
      <div className="flex w-full min-w-0 gap-2 overflow-x-auto px-4 py-3 md:h-full md:flex-col md:overflow-y-auto md:overflow-x-hidden">
        {boards.map((board) => {
          const selected = board.handler === selectedHandler;
          return (
            <button
              key={board.handler}
              type="button"
              onClick={() => onSelect(board.handler)}
              className={cn(
                "min-w-[14rem] max-w-[16rem] rounded-md border px-3 py-2 text-left transition-colors md:min-w-0 md:max-w-none",
                selected
                  ? "border-primary bg-primary/10 text-foreground"
                  : "border-border bg-card/40 hover:bg-accent/50",
              )}
            >
              <div className="flex min-w-0 items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-mono text-sm">@{board.handler}</span>
                <span
                  className="min-w-0 max-w-[55%] shrink truncate rounded bg-muted px-1.5 py-0.5 text-[10px] text-muted-foreground"
                  title={board.status}
                >
                  {board.status}
                </span>
              </div>
              {board.summary && (
                <p className="mt-1 line-clamp-2 break-words text-xs text-muted-foreground">
                  {board.summary}
                </p>
              )}
              <p className="mt-1 truncate text-[10px] text-muted-foreground/80">
                {formatDateTime(board.updated_at, timezone)}
              </p>
            </button>
          );
        })}
      </div>
    </aside>
  );
}

function BoardDetail({
  board,
  summary,
  loading,
}: {
  board: BoardReadResponse | null;
  summary: BoardSummary | null;
  loading: boolean;
}) {
  const timezone = useTimezoneStore((s) => s.timezone);
  if (!board) {
    return (
      <div className="flex min-h-0 flex-1 items-center justify-center p-8 text-sm text-muted-foreground">
        {loading ? "Loading board..." : "Select a board"}
      </div>
    );
  }

  const meta = board.meta;
  // v1 transition: wire field is `tags`. Internal Rust field is `labels`
  // (serde rename for cross-version compat). v2 may switch wire to `labels`.
  const tags = meta.tags.length > 0 ? meta.tags : (summary?.tags ?? []);
  return (
    <section className="min-h-0 overflow-y-auto px-4 py-4 md:px-6">
      <div className="mx-auto flex max-w-4xl flex-col gap-5">
        <header className="border-b border-border pb-4">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="min-w-0 break-all font-mono text-xl font-semibold">
              @{board.handler}
            </h2>
            <Badge variant="secondary">{meta.status}</Badge>
          </div>
          {meta.summary && (
            <p className="mt-2 max-w-3xl break-words text-sm text-muted-foreground">
              {meta.summary}
            </p>
          )}
          <div className="mt-3 flex flex-wrap items-center gap-2 text-xs text-muted-foreground">
            <span>{formatDateTime(meta.updated_at, timezone)}</span>
            {tags.map((tag) => (
              <Badge key={tag} variant="outline" className="max-w-full truncate">
                {tag}
              </Badge>
            ))}
          </div>
        </header>
        <MarkdownBody body={board.body} />
      </div>
    </section>
  );
}

function MarkdownBody({ body }: { body: string }) {
  const blocks = useMemo(() => parseBoardBody(body), [body]);
  if (blocks.length === 0) {
    return <p className="text-sm text-muted-foreground">No board content.</p>;
  }

  return (
    <div className="space-y-5">
      {blocks.map((block, idx) => (
        <section key={`${block.heading ?? "intro"}-${idx}`} className="min-w-0">
          {block.heading && (
            <h3 className="mb-2 break-words text-base font-semibold">
              {block.heading}
            </h3>
          )}
          {block.body && (
            <pre className="whitespace-pre-wrap break-words font-sans text-sm leading-6 text-foreground/90">
              {block.body}
            </pre>
          )}
        </section>
      ))}
    </div>
  );
}

function parseBoardBody(body: string): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  let current: MarkdownBlock = { heading: null, body: "" };

  for (const line of body.split("\n")) {
    if (line.startsWith("## ")) {
      if (current.heading || current.body.trim()) {
        blocks.push({ ...current, body: current.body.trim() });
      }
      current = { heading: line.slice(3).trim(), body: "" };
    } else {
      current.body += `${line}\n`;
    }
  }

  if (current.heading || current.body.trim()) {
    blocks.push({ ...current, body: current.body.trim() });
  }
  return blocks;
}

function EmptyBoards() {
  return (
    <div className="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
      <p className="text-base font-medium">No boards yet</p>
      <p className="max-w-sm text-sm text-muted-foreground">
        Boards appear here after someone initializes or publishes a showboard.
      </p>
    </div>
  );
}
