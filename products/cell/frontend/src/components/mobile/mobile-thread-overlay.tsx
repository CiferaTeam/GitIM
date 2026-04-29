import { useEffect, useState, useMemo } from "react";
import { X, MessageSquare, ArrowLeft } from "lucide-react";
import type { Message } from "../../lib/types";
import { formatTimestamp } from "../../lib/types";
import { cn } from "../../lib/utils";
import { MessageBody } from "../chat/message-body";

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

interface MobileThreadOverlayProps {
  root: Message | null;
  messages: Message[];
  onClose: () => void;
  onReplyInThread: (msg: Message) => void;
}

export function MobileThreadOverlay({ root, messages, onClose, onReplyInThread }: MobileThreadOverlayProps) {
  const [mounted, setMounted] = useState(!!root);

  useEffect(() => {
    if (root) setMounted(true);
    else {
      const t = setTimeout(() => setMounted(false), 250);
      return () => clearTimeout(t);
    }
  }, [root]);

  const msgByLine = useMemo(() => {
    const map = new Map<number, Message>();
    for (const msg of messages) {
      if (msg.type !== "event") map.set(msg.line_number, msg);
    }
    return map;
  }, [messages]);

  if (!mounted || !root) return null;

  return (
    <div
      className={cn(
        "fixed inset-0 z-50 bg-background flex flex-col md:hidden",
        "transition-transform duration-250 ease-[cubic-bezier(0.32,0.72,0,1)]",
        root ? "translate-x-0" : "translate-x-full"
      )}
    >
      <div className="h-14 border-b border-border flex items-center gap-3 px-4 shrink-0 bg-card/60">
        <button onClick={onClose} className="p-2 -ml-2 rounded-lg hover:bg-surface transition-colors active:scale-90">
          <ArrowLeft className="size-5 text-text-muted" />
        </button>
        <div className="flex items-center gap-2 min-w-0 flex-1">
          <MessageSquare className="size-4 text-primary shrink-0" />
          <span className="text-sm font-medium truncate text-foreground">{root.body}</span>
        </div>
        <button onClick={onClose} className="p-2 rounded-lg hover:bg-surface transition-colors active:scale-90">
          <X className="size-5 text-text-muted" />
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-3 flex flex-col gap-2">
        {messages.map((msg) => {
          if (msg.type === "event") return null;
          const isRoot = msg.line_number === root.line_number;
          const parent = msg.point_to > 0 ? msgByLine.get(msg.point_to) : null;

          return (
            <div
              key={msg.line_number}
              className={cn(
                "group rounded-xl px-3 py-3 text-sm transition-colors",
                isRoot ? "bg-surface border border-border" : ""
              )}
            >
              {isRoot && (
                <div className="text-[10px] text-primary mb-1.5 font-semibold uppercase tracking-wider">
                  Root
                </div>
              )}

              {msg.point_to > 0 && parent && (
                <div className="mb-1.5 border-l-2 border-text-muted/40 pl-2.5 text-xs text-text-muted py-0.5 rounded-r-md bg-surface/30">
                  <span className="font-medium">@{parent.author}: </span>
                  <span>{parent.body.length > 60 ? parent.body.slice(0, 60) + "…" : parent.body}</span>
                </div>
              )}

              <div className="flex items-center gap-2 mb-1">
                <div
                  className="shrink-0 w-7 h-7 rounded-full flex items-center justify-center text-[10px] font-bold text-white"
                  style={{ backgroundColor: avatarColor(msg.author) }}
                >
                  {initials(msg.author)}
                </div>
                <div className="flex items-baseline gap-2 min-w-0">
                  <span className="font-medium text-foreground truncate text-sm">@{msg.author}</span>
                  <span className="text-[11px] text-text-muted">{formatTimestamp(msg.timestamp)}</span>
                </div>
              </div>

              <div className="leading-relaxed text-foreground/95 pl-9 text-[15px]">
                <MessageBody body={msg.body} />
              </div>

              {!isRoot && (
                <div className="mt-1.5 pl-9">
                  <button
                    onClick={() => onReplyInThread(msg)}
                    className="flex items-center gap-1 px-2 py-1 text-xs text-text-muted hover:text-foreground hover:bg-surface-hover rounded-md transition-colors"
                  >
                    <MessageSquare className="h-3 w-3" />
                    <span>Reply</span>
                  </button>
                </div>
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}
