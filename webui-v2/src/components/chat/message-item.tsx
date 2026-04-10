import { useRef } from "react";
import { MessageSquare, GitBranch, Copy, Check } from "lucide-react";
import type { Message } from "../../lib/types";
import { formatTimestamp } from "../../lib/types";
import { cn } from "../../lib/utils";

interface MessageItemProps {
  message: Message;
  replyTarget: Message | null;
  onReply: (msg: Message) => void;
  onShowThread: (msg: Message) => void;
  isReplying: boolean;
  highlight: boolean;
  onScrollTo: (lineNumber: number) => void;
  onCopy: (body: string, lineNumber: number) => void;
  copied: boolean;
}

const STATUS_LABELS: Record<string, string> = {
  sending: "Sending...",
  sent: "Sent ✓",
  failed: "Failed ✗",
};

export function MessageItem({
  message,
  replyTarget,
  onReply,
  onShowThread,
  isReplying,
  highlight,
  onScrollTo,
  onCopy,
  copied,
}: MessageItemProps) {
  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const isPending = !!message._pendingId && message._status === "sending";
  const isFailed = message._status === "failed";

  function handleClick() {
    // Don't trigger on pending messages or text selection
    if (isPending) return;
    const selection = window.getSelection();
    if (selection && selection.toString().length > 0) return;

    clickTimerRef.current = setTimeout(() => {
      onReply(message);
      clickTimerRef.current = null;
    }, 250);
  }

  function handleDoubleClick() {
    if (isPending) return;
    if (clickTimerRef.current) {
      clearTimeout(clickTimerRef.current);
      clickTimerRef.current = null;
    }
    onShowThread(message);
  }

  const statusLabel = message._status ? STATUS_LABELS[message._status] : null;

  return (
    <div
      data-line={message.line_number}
      className={cn(
        "group relative rounded-md px-3 py-1.5 transition-all",
        isPending && "opacity-50",
        isFailed && "border border-destructive text-destructive",
        isReplying && "border-l-2 border-primary/60 pl-2",
        highlight && "bg-accent/20 ring-1 ring-accent/40"
      )}
    >
      {/* Hover actions bar */}
      {!isPending && (
        <div className="absolute right-2 top-1.5 hidden group-hover:flex items-center gap-1 bg-background/95 border rounded-md shadow-sm px-1 py-0.5 z-10">
          <button
            onClick={() => onReply(message)}
            className="flex items-center gap-1 px-1.5 py-0.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded"
            title="Reply"
          >
            <MessageSquare className="h-3 w-3" />
            <span>Reply</span>
          </button>
          <button
            onClick={() => onShowThread(message)}
            className="flex items-center gap-1 px-1.5 py-0.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded"
            title="Thread"
          >
            <GitBranch className="h-3 w-3" />
            <span>Thread</span>
          </button>
          <button
            onClick={() => onCopy(message.body, message.line_number)}
            className="flex items-center gap-1 px-1.5 py-0.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded"
            title="Copy"
          >
            {copied ? (
              <Check className="h-3 w-3 text-green-500" />
            ) : (
              <Copy className="h-3 w-3" />
            )}
            <span>{copied ? "Copied" : "Copy"}</span>
          </button>
        </div>
      )}

      {/* Message header */}
      <div className="flex items-baseline gap-2 mb-0.5">
        <span className="font-medium text-sm">@{message.author}</span>
        <span className="text-xs text-muted-foreground">
          {formatTimestamp(message.timestamp)}
        </span>
        {statusLabel && (
          <span
            className={cn(
              "text-xs",
              isFailed ? "text-destructive" : "text-muted-foreground"
            )}
          >
            {statusLabel}
          </span>
        )}
      </div>

      {/* Reply reference */}
      {message.point_to > 0 && replyTarget && (
        <button
          onClick={() => onScrollTo(message.point_to)}
          className="mb-1 flex items-start gap-1.5 text-left w-full"
        >
          <div className="border-l-2 border-muted-foreground/40 pl-2 text-xs text-muted-foreground hover:text-foreground transition-colors">
            <span className="font-medium">@{replyTarget.author}: </span>
            <span>
              {replyTarget.body.length > 60
                ? replyTarget.body.slice(0, 60) + "…"
                : replyTarget.body}
            </span>
          </div>
        </button>
      )}

      {/* Message body */}
      <p
        className="text-sm cursor-pointer select-text leading-relaxed"
        onClick={handleClick}
        onDoubleClick={handleDoubleClick}
      >
        {message.body}
      </p>
    </div>
  );
}
