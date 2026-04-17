import { useCallback, useEffect, useMemo, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { useNavigate, useParams } from "react-router";
import { toast } from "sonner";
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
import * as client from "@/lib/client";
import type { ApiResponse, Card, CardStatus, Message } from "@/lib/types";
import { nowTimestamp } from "@/lib/types";
import { CardMetaBar } from "./card-meta-bar";

type LoadStatus = "loading" | "ok" | "not_found" | "error";

export function CardDetail() {
  const params = useParams();
  const navigate = useNavigate();
  const channel = params.channel ?? "";
  const cardId = params.card_id ?? "";

  const currentUser = useChatStore((s) => s.currentUser);
  const users = useChatStore((s) => s.users);
  const agents = useAgentStore((s) => s.agents);

  const card = useCardStore((s) => selectCardById(s, channel, cardId));
  const upsertCard = useCardStore((s) => s.upsertCard);
  const setCardMessages = useCardStore((s) => s.setCardMessages);
  const addPendingCardMessage = useCardStore((s) => s.addPendingCardMessage);
  const markPendingCardSent = useCardStore((s) => s.markPendingCardSent);
  const markPendingCardFailed = useCardStore((s) => s.markPendingCardFailed);
  const markCardInFlight = useCardStore((s) => s.markCardInFlight);
  const unmarkCardInFlight = useCardStore((s) => s.unmarkCardInFlight);

  const pathKey = useMemo(() => cardPathKey(channel, cardId), [channel, cardId]);
  const scopeKey = useMemo(() => cardScopeKey(channel, cardId), [channel, cardId]);

  const messages = useCardStore(
    (s) => s.cardMessagesByPath[pathKey] ?? [],
  );

  const [loadStatus, setLoadStatus] = useState<LoadStatus>("loading");
  const [loadError, setLoadError] = useState<string | null>(null);
  const [replyTo, setReplyTo] = useState<Message | null>(null);
  const [highlightLine, setHighlightLine] = useState<number | null>(null);
  const [pendingScrollLine, setPendingScrollLine] = useState<number | null>(null);

  const mentionCandidates = useMemo(() => {
    const set = new Set<string>([...users, ...agents.map((a) => a.id)]);
    return [...set];
  }, [users, agents]);

  useEffect(() => {
    let aborted = false;
    // Intentional: set loading state synchronously when (channel, cardId) change
    // so the fetch-then-update cycle has a correct UI story.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLoadStatus("loading");
    setLoadError(null);
    (async () => {
      const res = await client.readCard(channel, cardId, { limit: 100 });
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
      upsertCard(res.data.meta);
      setCardMessages(pathKey, res.data.entries);
      setLoadStatus("ok");
    })();
    return () => {
      aborted = true;
    };
  }, [channel, cardId, pathKey, upsertCard, setCardMessages]);

  const handleUpdate = useCallback(
    async (patch: {
      status?: CardStatus;
      labels?: string[];
      assignee?: string | null;
    }) => {
      if (!card) return;
      const prev = card;
      const next: Card = {
        ...card,
        ...(patch.status !== undefined && { status: patch.status }),
        ...(patch.labels !== undefined && { labels: patch.labels }),
        ...(patch.assignee !== undefined && { assignee: patch.assignee }),
        updated_at: nowTimestamp(),
      };
      upsertCard(next);
      // Use channel/cardId from route params — `card.*` could be stale after the
      // optimistic upsertCard above if any poll tick landed between reads.
      markCardInFlight(channel, cardId);
      const res = await client.updateCard(channel, cardId, patch);
      unmarkCardInFlight(channel, cardId);
      if (!res.ok) {
        upsertCard(prev);
        toast.error(`Update failed: ${res.error ?? "unknown"}`);
      }
    },
    [card, upsertCard, channel, cardId, markCardInFlight, unmarkCardInFlight],
  );

  const handleSend = useCallback(
    async (body: string, pointTo: number): Promise<ApiResponse> => {
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
      channel,
      cardId,
      pathKey,
      currentUser,
      addPendingCardMessage,
      markPendingCardSent,
      markPendingCardFailed,
    ],
  );

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
          className="mt-2 text-xs text-[#60a5fa] hover:underline"
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
      </div>

      <CardMetaBar card={card} onUpdate={handleUpdate} />

      <MessageList
        messages={messages}
        scopeKey={scopeKey}
        replyTo={replyTo}
        highlightLine={highlightLine}
        pendingScrollLine={pendingScrollLine}
        onHighlightLineChange={setHighlightLine}
        onPendingScrollClear={() => setPendingScrollLine(null)}
        emptyHint="Write the first note…"
        onReply={setReplyTo}
        onShowThread={() => {
          /* thread panel not wired for card detail yet */
        }}
      />

      <InputArea
        scopeKey={scopeKey}
        replyTo={replyTo}
        onReplyToChange={setReplyTo}
        mentionCandidates={mentionCandidates}
        onSend={handleSend}
        placeholder="Write a note (Enter to send, Shift+Enter for newline)"
      />
    </div>
  );
}
