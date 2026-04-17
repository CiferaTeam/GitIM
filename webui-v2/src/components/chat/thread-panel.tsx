import { useMemo } from "react";
import { X, MessageSquare } from "lucide-react";
import type { Message } from "../../lib/types";
import { formatTimestamp } from "../../lib/types";
import { cn } from "../../lib/utils";
import { MessageBody } from "./message-body";

interface ThreadPanelProps {
  root: Message | null;
  messages: Message[];
  onClose: () => void;
  onReplyInThread: (msg: Message) => void;
  onMentionClick?: (handler: string, event: React.MouseEvent) => void;
  onChannelClick?: (channel: string) => void;
  onMessageLinkClick?: (channel: string, line: number) => void;
  onUserProfileClick?: (handler: string, event: React.MouseEvent) => void;
}

export function ThreadPanel({
  root,
  messages,
  onClose,
  onReplyInThread,
  onMentionClick,
  onChannelClick,
  onMessageLinkClick,
  onUserProfileClick,
}: ThreadPanelProps) {
  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const msg of messages) {
      if (msg.type !== "event") map.set(msg.line_number, msg);
    }
    return map;
  }, [messages]);

  if (!root) return null;

  return (
    <div className="w-80 shrink-0 border-l border-border/60 flex flex-col h-full bg-background">
      {/* Header */}
      <div className="h-12 border-b border-border/60 flex items-center justify-between px-4 overflow-hidden">
        <span className="text-sm font-medium truncate mr-2">{root.body}</span>
        <button
          onClick={onClose}
          className="p-1 rounded hover:bg-muted transition-colors text-muted-foreground hover:text-foreground"
          aria-label="Close thread"
        >
          <X className="h-4 w-4" />
        </button>
      </div>

      {/* Thread messages */}
      <div className="flex-1 overflow-y-auto p-3 flex flex-col gap-1">
        {messages.map((msg) => {
          if (msg.type === "event") return null;
          const isRoot = msg.line_number === root.line_number;
          const parent =
            msg.point_to > 0 ? msgByLine.get(msg.point_to) : null;

          return (
            <div
              key={msg.line_number}
              className={cn(
                "group rounded-md px-3 py-2 text-sm transition-colors",
                isRoot
                  ? "bg-muted/60 border border-border/60"
                  : "hover:bg-muted/40"
              )}
            >
              {isRoot && (
                <div className="text-xs text-muted-foreground mb-1 font-medium uppercase tracking-wide">
                  Root
                </div>
              )}

              {msg.point_to > 0 && parent && (
                <div className="mb-1.5 border-l-2 border-muted-foreground/40 pl-2 text-xs text-muted-foreground">
                  <span className="font-medium">@{parent.author}: </span>
                  <span>
                    {parent.body.length > 60
                      ? parent.body.slice(0, 60) + "…"
                      : parent.body}
                  </span>
                </div>
              )}

              <div className="flex items-baseline gap-2 mb-0.5">
                <span className="font-medium">@{msg.author}</span>
                <span className="text-xs text-muted-foreground">
                  {formatTimestamp(msg.timestamp)}
                </span>
              </div>

              <div className="leading-relaxed text-foreground">
                <MessageBody
                  body={msg.body}
                  onMentionClick={onMentionClick}
                  onChannelClick={onChannelClick}
                  onMessageLinkClick={onMessageLinkClick}
                  onUserProfileClick={onUserProfileClick}
                />
              </div>

              <div className="mt-1.5 hidden group-hover:flex">
                <button
                  onClick={() => onReplyInThread(msg)}
                  className="flex items-center gap-1 px-1.5 py-0.5 text-xs text-muted-foreground hover:text-foreground hover:bg-muted rounded transition-colors"
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
