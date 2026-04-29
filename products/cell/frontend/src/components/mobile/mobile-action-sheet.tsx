import { useEffect, useState } from "react";
import { MessageSquare, GitBranch, Copy, X } from "lucide-react";
import type { Message } from "../../lib/types";
import { cn } from "../../lib/utils";

interface MobileActionSheetProps {
  message: Message | null;
  onClose: () => void;
  onReply: (msg: Message) => void;
  onShowThread: (msg: Message) => void;
}

export function MobileActionSheet({ message, onClose, onReply, onShowThread }: MobileActionSheetProps) {
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    if (message) {
      setVisible(true);
    } else {
      setVisible(false);
    }
  }, [message]);

  if (!message) return null;

  function handleReply() {
    onReply(message);
    onClose();
  }

  function handleThread() {
    onShowThread(message);
    onClose();
  }

  function handleCopy() {
    navigator.clipboard.writeText(message.body).catch(() => {});
    onClose();
  }

  return (
    <div
      className={cn(
        "fixed inset-0 z-[60] flex flex-col justify-end transition-opacity duration-200",
        visible ? "opacity-100" : "opacity-0 pointer-events-none"
      )}
    >
      <div className="absolute inset-0 bg-black/40 backdrop-blur-sm" onClick={onClose} />
      <div
        className={cn(
          "relative bg-card rounded-t-2xl border-t border-border transition-transform duration-200 ease-[cubic-bezier(0.32,0.72,0,1)]",
          visible ? "translate-y-0" : "translate-y-full"
        )}
      >
        <div className="flex justify-center pt-3 pb-1">
          <div className="w-10 h-1 rounded-full bg-border" />
        </div>

        <div className="px-4 pb-3 border-b border-border/60">
          <p className="text-xs text-text-muted mb-1">@{message.author}</p>
          <p className="text-sm text-foreground line-clamp-2">{message.body}</p>
        </div>

        <div className="p-2 space-y-1">
          <button
            onClick={handleReply}
            className="w-full flex items-center gap-3 px-4 py-3.5 rounded-xl text-left text-foreground hover:bg-surface transition-colors active:scale-[0.98]"
          >
            <MessageSquare className="size-5 text-primary" />
            <span className="text-base">Reply</span>
          </button>
          <button
            onClick={handleThread}
            className="w-full flex items-center gap-3 px-4 py-3.5 rounded-xl text-left text-foreground hover:bg-surface transition-colors active:scale-[0.98]"
          >
            <GitBranch className="size-5 text-primary" />
            <span className="text-base">View Thread</span>
          </button>
          <button
            onClick={handleCopy}
            className="w-full flex items-center gap-3 px-4 py-3.5 rounded-xl text-left text-foreground hover:bg-surface transition-colors active:scale-[0.98]"
          >
            <Copy className="size-5 text-primary" />
            <span className="text-base">Copy Text</span>
          </button>
        </div>

        <div className="p-3 border-t border-border/60">
          <button
            onClick={onClose}
            className="w-full py-3.5 rounded-xl bg-surface text-foreground font-medium text-center hover:bg-surface-hover transition-colors active:scale-[0.98]"
          >
            Cancel
          </button>
        </div>
        <div className="h-[env(safe-area-inset-bottom)]" />
      </div>
    </div>
  );
}
