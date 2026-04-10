import { useRef, useState } from "react";
import { useChatStore } from "../../hooks/use-chat-store";
import type { Channel } from "../../lib/types";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import { Input } from "../ui/input";

interface SidebarProps {
  onChannelSelect: (name: string) => void;
  onStartDm: (targetUser: string) => void;
}

function dmDisplayName(channel: Channel, currentUser: string): string {
  const parts = channel.name.split("--");
  if (parts.length !== 2) return channel.name;
  const [a, b] = parts;
  // Self-DM
  if (a === b || (a === currentUser && b === currentUser)) {
    return `${currentUser} (me)`;
  }
  // Current user is a member — show the other side
  if (a === currentUser) return b;
  if (b === currentUser) return a;
  // Admin view — neither side is current user
  return `${a} \u2194 ${b}`;
}

function isSelfDm(channel: Channel, currentUser: string): boolean {
  const parts = channel.name.split("--");
  return parts.length === 2 && parts[0] === currentUser && parts[1] === currentUser;
}

export function Sidebar({ onChannelSelect, onStartDm }: SidebarProps) {
  const currentUser = useChatStore((s) => s.currentUser);
  const channels = useChatStore((s) => s.channels);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const users = useChatStore((s) => s.users);

  const [dmSearchOpen, setDmSearchOpen] = useState(false);
  const [dmQuery, setDmQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const regularChannels = channels.filter((c) => c.kind === "channel");
  const dmChannels = channels
    .filter((c) => c.kind === "dm")
    .sort((a, b) => {
      // Self-DM to top
      const aSelf = isSelfDm(a, currentUser);
      const bSelf = isSelfDm(b, currentUser);
      if (aSelf && !bSelf) return -1;
      if (!aSelf && bSelf) return 1;
      return a.name.localeCompare(b.name);
    });

  const filteredUsers = dmQuery.trim()
    ? users.filter(
        (u) =>
          u.toLowerCase().includes(dmQuery.toLowerCase()) && u !== currentUser
      )
    : users.filter((u) => u !== currentUser);

  function openDmSearch() {
    setDmSearchOpen(true);
    setDmQuery("");
    // Focus after render
    setTimeout(() => inputRef.current?.focus(), 0);
  }

  function handleUserSelect(user: string) {
    setDmSearchOpen(false);
    setDmQuery("");
    onStartDm(user);
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Escape") {
      setDmSearchOpen(false);
      setDmQuery("");
    }
  }

  return (
    <div className="w-56 shrink-0 border-r border-border/60 bg-muted/30 flex flex-col overflow-y-auto">
      {/* Channels section */}
      <div className="px-3 pt-4 pb-2">
        <p className="text-[10px] font-semibold uppercase text-muted-foreground tracking-widest mb-2 px-2">
          Channels
        </p>
        <ul className="space-y-0.5">
          {regularChannels.map((ch) => (
            <ChannelItem
              key={ch.name}
              label={`# ${ch.name}`}
              unread={ch.unreadCount}
              active={currentChannel === ch.name}
              onClick={() => onChannelSelect(ch.name)}
            />
          ))}
        </ul>
      </div>

      {/* DMs section */}
      <div className="px-3 pt-3 pb-4">
        <div className="flex items-center justify-between mb-2 px-2">
          <p className="text-[10px] font-semibold uppercase text-muted-foreground tracking-widest">
            Direct Messages
          </p>
          <Button
            variant="ghost"
            size="icon-xs"
            title="New DM"
            onClick={openDmSearch}
            className="text-muted-foreground hover:text-foreground"
          >
            <span className="text-base leading-none">+</span>
          </Button>
        </div>

        {/* Inline search */}
        {dmSearchOpen && (
          <div className="mb-2 relative px-1">
            <Input
              ref={inputRef}
              placeholder="Search users..."
              value={dmQuery}
              onChange={(e) => setDmQuery(e.target.value)}
              onKeyDown={handleKeyDown}
              className="h-7 text-xs"
            />
            {filteredUsers.length > 0 && (
              <ul className="absolute z-50 top-full left-0 right-0 mt-1 rounded-md border bg-popover shadow-lg max-h-40 overflow-y-auto">
                {filteredUsers.map((u) => (
                  <li
                    key={u}
                    className="px-3 py-1.5 text-sm cursor-pointer hover:bg-accent hover:text-accent-foreground transition-colors"
                    onMouseDown={() => handleUserSelect(u)}
                  >
                    @{u}
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}

        <ul className="space-y-0.5">
          {dmChannels.map((ch) => {
            const label = `@ ${dmDisplayName(ch, currentUser)}`;
            return (
              <ChannelItem
                key={ch.name}
                label={label}
                unread={ch.unreadCount}
                active={currentChannel === ch.name}
                onClick={() => onChannelSelect(ch.name)}
              />
            );
          })}
        </ul>
      </div>
    </div>
  );
}

interface ChannelItemProps {
  label: string;
  unread: number;
  active: boolean;
  onClick: () => void;
}

function ChannelItem({ label, unread, active, onClick }: ChannelItemProps) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={[
          "w-full flex items-center justify-between rounded-md px-2 py-1.5 text-[13px] text-left transition-colors",
          active
            ? "bg-accent text-accent-foreground font-medium"
            : "hover:bg-accent/40 text-muted-foreground hover:text-foreground",
          unread > 0 && !active ? "text-foreground font-medium" : "",
        ].join(" ")}
      >
        <span className="truncate">{label}</span>
        {unread > 0 && (
          <Badge variant="default" className="ml-1.5 text-[10px] px-1.5 py-0 h-4 min-w-4 font-mono">
            {unread}
          </Badge>
        )}
      </button>
    </li>
  );
}
