import { useMemo } from "react";
import { X, MessageSquare } from "lucide-react";
import { useChatStore } from "../../hooks/use-chat-store";
import type { Message } from "../../lib/types";
import { formatTimestamp } from "../../lib/types";
import { cn } from "../../lib/utils";
import { MessageBody } from "./message-body";

interface ThreadPanelProps {
  onReplyInThread: (msg: Message) => void;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

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

export function ThreadPanel({
  onReplyInThread,
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: ThreadPanelProps) {
  const threadRoot = useChatStore((s) => s.threadRoot);
  const threadMessages = useChatStore((s) => s.threadMessages);
  const setThreadRoot = useChatStore((s) => s.setThreadRoot);

  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const msg of threadMessages) {
      if (msg.type !== "event") map.set(msg.line_number, msg);
    }
    return map;
  }, [threadMessages]);

  if (!threadRoot) return null;

  return (
    <div className="w-80 shrink-0 border-l border-border flex flex-col h-full bg-card/40">
      {/* Header */}
      <div className="h-12 border-b border-border flex items-center justify-between px-4 overflow-hidden bg-card/60">
        <div className="flex items-center gap-2 min-w-0">
          <MessageSquare className="size-4 text-primary shrink-0" />
          <span className="text-sm font-medium truncate text-foreground">{threadRoot.body}</span>
        </div>
        <button
          onClick={() => setThreadRoot(null)}
          className="p-1.5 rounded-md hover:bg-surface-hover transition-colors text-text-muted hover:text-foreground shrink-0"
          aria-label="Close thread"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {/* Thread messages */}
      <div className="flex-1 overflow-y-auto p-3 flex flex-col gap-2">
        {threadMessages.map((msg) => {
          if (msg.type === "event") return null;
          const isRoot = msg.line_number === threadRoot.line_number;
          const parent =
            msg.point_to > 0 ? msgByLine.get(msg.point_to) : null;

          return (
            <div
              key={msg.line_number}
              className={cn(
                "group rounded-lg px-3 py-2 text-sm transition-colors",
                isRoot
                  ? "bg-surface border border-border"
                  : "hover:bg-surface/40"
              )}
            >
              {/* Root label */}
              {isRoot && (
                <div className="text-[10px] text-primary mb-1.5 font-semibold uppercase tracking-wider">
                  Root
                </div>
              )}

              {/* Reply reference */}
              {msg.point_to > 0 && parent && (
                <div className="mb-1.5 border-l-2 border-text-muted/40 pl-2.5 text-xs text-text-muted py-0.5 rounded-r-md bg-surface/30">
                  <span className="font-medium">@{parent.author}: </span>
                  <span>
                    {parent.body.length > 60
                      ? parent.body.slice(0, 60) + "…"
                      : parent.body}
                  </span>
                </div>
              )}

              {/* Header with avatar */}
              <div className="flex items-center gap-2 mb-1">
                <div
                  className="shrink-0 w-6 h-6 rounded-full flex items-center justify-center text-[9px] font-bold text-white"
                  style={{ backgroundColor: avatarColor(msg.author) }}
                >
                  {initials(msg.author)}
                </div>
                <div className="flex items-baseline gap-2 min-w-0">
                  <span className="font-medium text-foreground truncate">@{msg.author}</span>
                  <span className="text-[11px] text-text-muted">
                    {formatTimestamp(msg.timestamp)}
                  </span>
                </div>
              </div>

              {/* Body */}
              <div className="leading-relaxed text-foreground/95 pl-8">
                <MessageBody
                  body={msg.body}
                  onMentionClick={onMentionClick}
                  onChannelClick={onChannelClick}
                  onMessageLinkClick={onMessageLinkClick}
                  onUserProfileClick={onUserProfileClick}
                />
              </div>

              {/* Reply button */}
              <div className="mt-1.5 hidden group-hover:flex pl-8">
                <button
                  onClick={() => onReplyInThread(msg)}
                  className="flex items-center gap-1 px-2 py-1 text-xs text-text-muted hover:text-foreground hover:bg-surface-hover rounded-md transition-colors"
                >
                  <MessageSquare className="h-3 w-3" />
                  <span>Reply</span>
                </button>
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
