import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Archive, ArchiveRestore, ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { InputArea } from "@/components/chat/input-area";
import { MessageList } from "@/components/chat/message-list";
import { useAgentStore } from "@/hooks/use-agent-store";
import {
  useCardStore,
  cardPathKey,
  cardScopeKey,
  selectCardById,
} from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { useWorkspaceStore } from "@/hooks/use-workspace-store";
import * as client from "@/lib/client";
import type { ApiResponse, Card, CardStatus, Message } from "@/lib/types";
import { nowTimestamp } from "@/lib/types";
import { CardMetaBar } from "./card-meta-bar";

type LoadStatus = "loading" | "ok" | "not_found" | "error";

// Stable empty-array reference: zustand compares selector output by Object.is,
// so `?? []` would return a fresh array every call and loop useSyncExternalStore.
const EMPTY_MESSAGES: Message[] = [];

export function CardDetail() {
  const params = useParams();
  const navigate = useNavigate();
  const channel = params.channel ?? "";
  const cardId = params.card_id ?? "";

  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const currentUser = useChatStore((s) => s.currentUser);
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);

  const activeCard = useCardStore((s) => selectCardById(s, channel, cardId));
  const archivedCard = useCardStore((s) =>
    s.archivedCards.find((c) => c.channel === channel && c.card_id === cardId),
  );
  // Pick from archived fallback so the drawer can still render card meta after
  // the archive move removed it from `cards`. `archived` state below is the
  // source of truth for UI; `card` just provides title/labels/etc to render.
  const card = activeCard ?? archivedCard;
  const upsertCard = useCardStore((s) => s.upsertCard);
  const upsertArchivedCard = useCardStore((s) => s.upsertArchivedCard);
  const setCardMessages = useCardStore((s) => s.setCardMessages);
  const addPendingCardMessage = useCardStore((s) => s.addPendingCardMessage);
  const markPendingCardSent = useCardStore((s) => s.markPendingCardSent);
  const markPendingCardFailed = useCardStore((s) => s.markPendingCardFailed);
  const markCardInFlight = useCardStore((s) => s.markCardInFlight);
  const unmarkCardInFlight = useCardStore((s) => s.unmarkCardInFlight);
  const markArchived = useCardStore((s) => s.markArchived);
  const markUnarchived = useCardStore((s) => s.markUnarchived);

  const pathKey = useMemo(() => cardPathKey(channel, cardId), [channel, cardId]);
  const scopeKey = useMemo(() => cardScopeKey(channel, cardId), [channel, cardId]);

  const messages = useCardStore(
    (s) => s.cardMessagesByPath[pathKey] ?? EMPTY_MESSAGES,
  );

  const [loadStatus, setLoadStatus] = useState<LoadStatus>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [archived, setArchived] = useState<boolean>(false);
  const [archiveInFlight, setArchiveInFlight] = useState<boolean>(false);
  const [replyTo, setReplyTo] = useState<Message | null>(null);
  const [highlightLine, setHighlightLine] = useState<number | null>(null);
  const [pendingScrollLine, setPendingScrollLine] = useState<number | null>(null);

  // Tracks whether this component is still mounted, for gating post-await
  // side-effects (setState, toast, navigate) in archive/unarchive handlers.
  // Prevents setState-on-unmounted warnings and orphan toasts when the user
  // navigates away mid-flight.
  const isMountedRef = useRef(true);
  useEffect(() => {
    isMountedRef.current = true;
    return () => {
      isMountedRef.current = false;
    };
  }, []);

  const mentionCandidates = useMemo(() => {
    const set = new Set<string>([...users, ...agents.map((a) => a.id)]);
    return [...set];
  }, [users, agents]);

  useEffect(() => {
    if (!activeSlug) return;
    let aborted = false;
    // Intentional: set loading state synchronously when (channel, cardId) change
    // so the fetch-then-update cycle has a correct UI story.
    // Also reset `archived` so navigating from an archived card to an active one
    // doesn't flash the banner + hidden input on the new card before the fetch
    // settles.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLoadStatus("loading");
    setLoadError(null);
    setArchived(false);
    (async () => {
      const res = await client.readCard(activeSlug, channel, cardId, { limit: 100 });
      if (aborted) return;
      if (!res.ok) {
        const err = res.error ?? "failed to load";
        if (err.includes("not found") || err.includes("invalid")) {
          setLoadStatus("not_found");
        } else {
          setLoadStatus("error");
          setLoadError(err);
        }
        return;
      }
      if (!res.data) {
        setLoadStatus("not_found");
        return;
      }
      setArchived(res.data.archived);
      // Cache meta into the appropriate bucket. For active cards, upsert into
      // `cards` so poll merges align. For archived, upsert into `archivedCards`
      // — otherwise direct-URL loads (bookmark / refresh / deep link) find
      // nothing in either bucket and the drawer falls into the "failed to
      // load" branch despite a successful fetch.
      if (res.data.archived) {
        upsertArchivedCard(res.data.meta);
      } else {
        upsertCard(res.data.meta);
      }
      setCardMessages(pathKey, res.data.entries);
      setLoadStatus("ok");
    })();
    return () => {
      aborted = true;
    };
  }, [activeSlug, channel, cardId, pathKey, upsertCard, upsertArchivedCard, setCardMessages]);

  const handleUpdate = useCallback(
    async (patch: {
      status?: CardStatus;
      labels?: string[];
      assignee?: string | null;
    }) => {
      if (!card || !activeSlug) return;
      const prev = card;
      const next: Card = {
        ...card,
        ...(patch.status !== undefined && { status: patch.status }),
        ...(patch.labels !== undefined && { labels: patch.labels }),
        ...(patch.assignee !== undefined && { assignee: patch.assignee }),
        updated_at: nowTimestamp(),
      };
      // Mark in-flight BEFORE optimistic upsert so the merge guard is always
      // tighter than any intervening poll tick, closing the theoretical race
      // where listCards() returns mid-edit with pre-patch state.
      markCardInFlight(channel, cardId);
      upsertCard(next);
      // Use channel/cardId from route params — `card.*` could be stale after
      // the optimistic upsertCard above if any poll tick landed between reads.
      const res = await client.updateCard(activeSlug, channel, cardId, patch);
      unmarkCardInFlight(channel, cardId);
      if (!res.ok) {
        upsertCard(prev);
        toast.error(`Update failed: ${res.error ?? "unknown"}`);
      }
    },
    [activeSlug, card, upsertCard, channel, cardId, markCardInFlight, unmarkCardInFlight],
  );

  const handleSend = useCallback(
    async (body: string, pointTo: number): Promise<ApiResponse> => {
      if (!activeSlug) {
        return { ok: false, error: "No workspace selected" };
      }
      const pendingId = `pending-${Date.now()}`;
      const pending: Message = {
        line_number: -1,
        point_to: pointTo ?? 0,
        author: currentUser,
        timestamp: nowTimestamp(),
        body,
        _status: "sending",
        _pendingId: pendingId,
      };
      addPendingCardMessage(pathKey, pending);
      const res = await client.sendCardMessage(
        activeSlug,
        channel,
        cardId,
        body,
        pointTo || undefined,
      );
      if (res.ok && res.data) {
        markPendingCardSent(pathKey, pendingId, res.data.line_number as number);
      } else {
        markPendingCardFailed(pathKey, pendingId);
      }
      return res;
    },
    [
      activeSlug,
      channel,
      cardId,
      pathKey,
      currentUser,
      addPendingCardMessage,
      markPendingCardSent,
      markPendingCardFailed,
    ],
  );

  const handleArchive = useCallback(async () => {
    if (archiveInFlight || !activeSlug) return;
    setArchiveInFlight(true);
    const res = await client.archiveCard(activeSlug, channel, cardId);
    // Gate all post-await effects — user may have navigated away mid-flight.
    if (!isMountedRef.current) return;
    setArchiveInFlight(false);
    if (!res.ok) {
      toast.error(`Failed to archive: ${res.error ?? "unknown"}`);
      return;
    }
    markArchived(channel, cardId);
    setArchived(true);
    toast.success("Card archived");
    // Close drawer — the card is gone from the active board. Nav back so the
    // user returns to whatever they came from (kanban or elsewhere).
    if (window.history.length > 1) {
      navigate(-1);
    } else {
      navigate("/cards");
    }
  }, [activeSlug, archiveInFlight, channel, cardId, markArchived, navigate]);

  const handleUnarchive = useCallback(async () => {
    if (archiveInFlight || !activeSlug) return;
    setArchiveInFlight(true);
    const res = await client.unarchiveCard(activeSlug, channel, cardId);
    // Gate all post-await effects — user may have navigated away mid-flight.
    if (!isMountedRef.current) return;
    setArchiveInFlight(false);
    if (!res.ok) {
      toast.error(`Failed to unarchive: ${res.error ?? "unknown"}`);
      return;
    }
    markUnarchived(channel, cardId);
    setArchived(false);
    toast.success("Card restored");
  }, [activeSlug, archiveInFlight, channel, cardId, markUnarchived]);

  function handleBack() {
    if (window.history.length > 1) {
      navigate(-1);
    } else {
      navigate("/cards");
    }
  }

  if (loadStatus === "loading") {
    return (
      <div className="flex items-center justify-center h-full text-sm text-muted-foreground">
        Loading…
      </div>
    );
  }

  if (loadStatus === "not_found") {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2">
        <p className="text-base font-medium">Card not found</p>
        <p className="text-sm text-muted-foreground">
          {channel}/{cardId}
        </p>
        <button
          onClick={() => navigate("/cards")}
          className="mt-2 text-xs text-primary hover:underline"
        >
          ← Back to cards
        </button>
      </div>
    );
  }

  if (loadStatus === "error" || !card) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2">
        <p className="text-base font-medium">Failed to load card</p>
        <p className="text-sm text-muted-foreground">{loadError ?? "unknown error"}</p>
      </div>
    );
  }

  return (
    <div className="flex flex-col h-full overflow-hidden">
      <div className="flex items-center gap-2 px-4 py-2 border-b border-border text-xs">
        <button
          onClick={handleBack}
          className="flex items-center gap-1 px-2 py-1 rounded hover:bg-muted text-muted-foreground hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" />
          <span>Back</span>
        </button>
        <span className="text-muted-foreground">
          #{channel} / <span className="font-mono">{cardId}</span>
        </span>
        <div className="ml-auto">
          {archived ? (
            <Button
              variant="default"
              size="xs"
              onClick={handleUnarchive}
              disabled={archiveInFlight}
              className="gap-1"
            >
              <ArchiveRestore className="h-3 w-3" />
              Unarchive
            </Button>
          ) : (
            <Button
              variant="ghost"
              size="xs"
              onClick={handleArchive}
              disabled={archiveInFlight}
              className="gap-1 text-muted-foreground hover:text-foreground"
              title="Archive this card"
            >
              <Archive className="h-3 w-3" />
              Archive
            </Button>
          )}
        </div>
      </div>

      {archived && (
        <div className="px-4 py-2 border-b border-border bg-muted/40 text-xs text-muted-foreground flex items-center gap-2">
          <Archive className="h-3.5 w-3.5 shrink-0" />
          <span>This card is archived. Edits are disabled.</span>
        </div>
      )}

      <CardMetaBar card={card} onUpdate={handleUpdate} disabled={archived} />

      <MessageList
        messages={messages}
        scopeKey={scopeKey}
        replyTo={replyTo}
        highlightLine={highlightLine}
        pendingScrollLine={pendingScrollLine}
        onHighlightLineChange={setHighlightLine}
        onPendingScrollClear={() => setPendingScrollLine(null)}
        emptyHint={archived ? "No notes to add — card is archived." : "Write the first note…"}
        onReply={setReplyTo}
        onShowThread={() => {
          /* thread panel not wired for card detail yet */
        }}
      />

      {!archived && (
        <InputArea
          scopeKey={scopeKey}
          replyTo={replyTo}
          onReplyToChange={setReplyTo}
          mentionCandidates={mentionCandidates}
          onSend={handleSend}
          placeholder="Write a note (Enter to send, Shift+Enter for newline)"
        />
      )}
    </div>
  );
}
