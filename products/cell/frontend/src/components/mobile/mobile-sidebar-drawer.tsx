import { X, Hash, AtSign, MessageSquare } from "lucide-react";
import { useChatStore } from "../../hooks/use-chat-store";
import { cn } from "../../lib/utils";
import type { Channel } from "../../lib/types";

function dmDisplayName(channel: Channel, currentUser: string): string {
  const parts = channel.name.split("--");
  if (parts.length !== 2) return channel.name;
  const [a, b] = parts;
  if (a === currentUser) return b;
  if (b === currentUser) return a;
  return `${a} / ${b}`;
}

function isMyDm(channel: Channel, currentUser: string): boolean {
  const parts = channel.name.split("--");
  return parts.length === 2 && (parts[0] === currentUser || parts[1] === currentUser);
}

interface MobileSidebarDrawerProps {
  open: boolean;
  onClose: () => void;
  onChannelSelect: (name: string) => void;
}

export function MobileSidebarDrawer({ open, onClose, onChannelSelect }: MobileSidebarDrawerProps) {
  const channels = useChatStore((s) => s.channels);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const currentUser = useChatStore((s) => s.currentUser);

  const regularChannels = channels.filter((c) => c.kind === "channel");
  const dmChannels = channels
    .filter((c) => c.kind === "dm")
    .sort((a, b) => {
      const aMy = isMyDm(a, currentUser);
      const bMy = isMyDm(b, currentUser);
      if (aMy && !bMy) return -1;
      if (!aMy && bMy) return 1;
      return a.name.localeCompare(b.name);
    });

  if (!open) return null;

  function handleSelect(name: string) {
    onChannelSelect(name);
    onClose();
  }

  return (
    <div className="fixed inset-0 z-50 flex md:hidden">
      <div
        className={cn(
          "absolute inset-0 bg-black/50 backdrop-blur-sm transition-opacity duration-300",
          "opacity-100"
        )}
        onClick={onClose}
      />
      <div
        className={cn(
          "relative w-[85%] max-w-sm h-full bg-card border-r border-border flex flex-col",
          "transition-transform duration-300 ease-[cubic-bezier(0.32,0.72,0,1)]",
          "translate-x-0"
        )}
      >
        <div className="h-14 border-b border-border flex items-center justify-between px-4 shrink-0 bg-card/80">
          <div className="flex items-center gap-2">
            <MessageSquare className="size-5 text-primary" />
            <span className="font-bold text-sm tracking-tight">
              GitIM<span className="text-primary">·</span>Cell
            </span>
          </div>
          <button
            onClick={onClose}
            className="p-2 rounded-lg hover:bg-surface transition-colors active:scale-90"
          >
            <X className="size-5 text-text-muted" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto">
          <div className="px-3 pt-4 pb-2">
            <p className="px-3 text-[11px] font-semibold uppercase text-text-muted tracking-wider mb-2">
              Channels
            </p>
            <div className="space-y-0.5">
              {regularChannels.map((ch) => (
                <button
                  key={ch.name}
                  onClick={() => handleSelect(ch.name)}
                  className={cn(
                    "w-full flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-left transition-colors active:scale-[0.98]",
                    currentChannel === ch.name
                      ? "bg-primary/15 text-primary font-medium"
                      : "text-text-secondary hover:bg-surface/60 hover:text-foreground",
                    ch.unreadCount > 0 && currentChannel !== ch.name && "text-foreground font-medium"
                  )}
                >
                  <Hash className="size-4 shrink-0" />
                  <span className="truncate flex-1 text-sm">{ch.name}</span>
                  {ch.unreadCount > 0 && (
                    <span className={cn(
                      "text-[11px] px-2 py-0.5 rounded-full font-mono shrink-0",
                      ch.hasMention
                        ? "bg-primary text-white"
                        : "bg-surface-hover text-foreground border border-border"
                    )}>
                      {ch.hasMention ? `${ch.unreadCount}@` : ch.unreadCount}
                    </span>
                  )}
                </button>
              ))}
            </div>
          </div>

          <div className="px-3 pt-3 pb-4 border-t border-border/60">
            <p className="px-3 text-[11px] font-semibold uppercase text-text-muted tracking-wider mb-2">
              Direct Messages
            </p>
            <div className="space-y-0.5">
              {dmChannels.map((ch) => {
                const label = dmDisplayName(ch, currentUser);
                return (
                  <button
                    key={ch.name}
                    onClick={() => handleSelect(ch.name)}
                    className={cn(
                      "w-full flex items-center gap-2.5 px-3 py-2.5 rounded-lg text-left transition-colors active:scale-[0.98]",
                      currentChannel === ch.name
                        ? "bg-primary/15 text-primary font-medium"
                        : "text-text-secondary hover:bg-surface/60 hover:text-foreground",
                      ch.unreadCount > 0 && currentChannel !== ch.name && "text-foreground font-medium"
                    )}
                  >
                    <AtSign className="size-4 shrink-0" />
                    <span className="truncate flex-1 text-sm">{label}</span>
                    {ch.unreadCount > 0 && (
                      <span className="text-[11px] px-2 py-0.5 rounded-full font-mono shrink-0 bg-surface-hover text-foreground border border-border">
                        {ch.unreadCount}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          </div>
        </div>

        <div className="px-4 py-3 border-t border-border/60 shrink-0">
          <span className="text-xs text-text-muted font-mono bg-surface px-3 py-1.5 rounded-lg border border-border inline-block">
            @{currentUser}
          </span>
        </div>
      </div>
    </div>
  );
}
