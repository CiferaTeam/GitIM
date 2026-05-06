import { useEffect, useRef } from "react";
import { MessageSquare, GitBranch, Copy, Check } from "lucide-react";
import type { Message } from "../../lib/types";
import { formatTimestamp } from "../../lib/types";
import { cn } from "../../lib/utils";
import { MessageBody } from "./message-body";

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
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

const STATUS_LABELS: Record<string, string> = {
  sending: "Sending...",
  sent: "Sent ✓",
  failed: "Failed ✗",
};

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

function avatarColor(name: string) {
  const hues = [210, 150, 30, 280, 340, 190, 45, 260];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = name.charCodeAt(i) + ((hash << 5) - hash);
  const hue = hues[Math.abs(hash) % hues.length];
  return `hsl(${hue} 70% 55%)`;
}

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
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: MessageItemProps) {
  const clickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const isPending = !!message._pendingId && message._status === "sending";

  useEffect(() => {
    return () => {
      if (clickTimerRef.current !== null) clearTimeout(clickTimerRef.current);
    };
  }, []);
  const isFailed = message._status === "failed";

  function handleClick() {
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
        "group relative rounded-lg px-3 py-2.5 transition-all duration-150",
        "hover:bg-surface/40",
        isPending && "opacity-40",
        isFailed && "border border-destructive/50 bg-destructive/5",
        isReplying && "border-l-2 border-ring/60 bg-muted/20",
        highlight && "bg-primary/10 ring-1 ring-primary/30"
      )}
    >
      {/* Hover actions bar */}
      {!isPending && (
        <div className="absolute right-2 top-1.5 hidden group-hover:flex items-center gap-0.5 bg-popover/95 backdrop-blur-md border border-border rounded-lg shadow-lg px-1 py-0.5 z-10">
          <button
            onClick={() => onReply(message)}
            className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-surface-hover rounded-md transition-colors"
            title="Reply"
          >
            <MessageSquare className="h-3 w-3" />
            <span>Reply</span>
          </button>
          <button
            onClick={() => onShowThread(message)}
            className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-surface-hover rounded-md transition-colors"
            title="Thread"
          >
            <GitBranch className="h-3 w-3" />
            <span>Thread</span>
          </button>
          <button
            onClick={() => onCopy(message.body, message.line_number)}
            className="flex items-center gap-1 px-2 py-1 text-xs text-muted-foreground hover:text-foreground hover:bg-surface-hover rounded-md transition-colors"
            title="Copy"
          >
            {copied ? (
              <Check className="h-3 w-3 text-success" />
            ) : (
              <Copy className="h-3 w-3" />
            )}
            <span>{copied ? "Copied" : "Copy"}</span>
          </button>
        </div>
      )}

      <div className="flex gap-3">
        {/* Avatar */}
        <div
          className="shrink-0 w-8 h-8 rounded-full flex items-center justify-center text-[10px] font-bold text-white shadow-sm"
          style={{ backgroundColor: avatarColor(message.author) }}
          title={message.author}
        >
          {initials(message.author)}
        </div>

        <div className="flex-1 min-w-0">
          {/* Message header */}
          <div className="flex items-baseline gap-2 mb-0.5">
            <span className="font-semibold text-sm text-foreground">@{message.author}</span>
            <span className="text-[11px] text-text-muted font-mono">
              {formatTimestamp(message.timestamp)}
            </span>
            {statusLabel && (
              <span
                className={cn(
                  "text-[11px]",
                  isFailed ? "text-destructive" : "text-text-muted"
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
              className="mb-1.5 flex items-start gap-1.5 text-left w-full group/reply"
            >
              <div className="border-l-2 border-text-muted/40 pl-2.5 py-0.5 text-xs text-text-muted group-hover/reply:text-foreground transition-colors rounded-r-md bg-surface/30 group-hover/reply:bg-surface/60">
                <span className="font-medium">@{replyTarget.author}: </span>
                <span>
                  {replyTarget.body.length > 60
                    ? replyTarget.body.slice(0, 60) + "..."
                    : replyTarget.body}
                </span>
              </div>
            </button>
          )}

          {/* Message body */}
          <div
            className="text-sm cursor-pointer select-text leading-relaxed text-foreground/95"
            onClick={handleClick}
            onDoubleClick={handleDoubleClick}
          >
            <MessageBody
              body={message.body}
              onMentionClick={onMentionClick}
              onChannelClick={onChannelClick}
              onMessageLinkClick={onMessageLinkClick}
              onUserProfileClick={onUserProfileClick}
            />
          </div>
        </div>
      </div>
    </div>
  );
}
