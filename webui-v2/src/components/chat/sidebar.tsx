import { useEffect, useRef, useState } from "react";
import { useChatStore } from "../../hooks/use-chat-store";
import * as client from "../../lib/client";
import type { Channel } from "../../lib/types";
import { AgentStatusPanel } from "./agent-status-panel";
import { Badge } from "../ui/badge";
import { Button } from "../ui/button";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogFooter,
} from "../ui/dialog";
import { Input } from "../ui/input";
import { Popover, PopoverTrigger, PopoverContent } from "../ui/popover";

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

  const setChannels = useChatStore((s) => s.setChannels);

  const [dmSearchOpen, setDmSearchOpen] = useState(false);
  const [dmQuery, setDmQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  // Create channel dialog state
  const [createOpen, setCreateOpen] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createDisplayName, setCreateDisplayName] = useState("");
  const [createIntro, setCreateIntro] = useState("");
  const [createError, setCreateError] = useState("");
  const [creating, setCreating] = useState(false);

  function resetCreateForm() {
    setCreateName("");
    setCreateDisplayName("");
    setCreateIntro("");
    setCreateError("");
    setCreating(false);
  }

  async function handleCreateChannel() {
    const name = createName.trim().toLowerCase();
    const validation = client.validateChannelName(name);
    if (validation) {
      setCreateError(validation);
      return;
    }
    setCreating(true);
    setCreateError("");
    let created = false;
    try {
      const res = await client.createChannel(
        name,
        createDisplayName.trim() || undefined,
        createIntro.trim() || undefined,
      );
      if (!res.ok) {
        setCreateError(res.error ?? "Failed to create channel");
        setCreating(false);
        return;
      }
      created = true;
    } catch {
      setCreateError("Network error — is the server running?");
      setCreating(false);
      return;
    }
    // Creation succeeded — refresh and navigate (failures here are non-fatal)
    try {
      const chRes = await client.channels();
      if (chRes.ok && chRes.data) {
        setChannels(chRes.data.channels as Channel[]);
      }
    } catch { /* refresh failure is non-fatal */ }
    resetCreateForm();
    setCreateOpen(false);
    onChannelSelect(name);
  }

  // Auto-focus input when popover opens
  useEffect(() => {
    if (dmSearchOpen) {
      setTimeout(() => inputRef.current?.focus(), 0);
    } else {
      setDmQuery("");
    }
  }, [dmSearchOpen]);

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

  function handleUserSelect(user: string) {
    setDmSearchOpen(false);
    onStartDm(user);
  }

  return (
    <div className="w-56 shrink-0 border-r border-border/60 bg-muted/30 flex flex-col overflow-y-auto">
      {/* Agent status panel */}
      <AgentStatusPanel />

      {/* Channels section */}
      <div className="px-3 pt-4 pb-2">
        <div className="flex items-center justify-between mb-2 px-2">
          <p className="text-[10px] font-semibold uppercase text-muted-foreground tracking-widest">
            Channels
          </p>
          <Button
            variant="ghost"
            size="icon-xs"
            title="Create channel"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setCreateOpen(true)}
          >
            <span className="text-base leading-none">+</span>
          </Button>
        </div>
        <ul className="space-y-0.5">
          {regularChannels.map((ch) => (
            <ChannelItem
              key={ch.name}
              label={`# ${ch.name}`}
              unread={ch.unreadCount}
              hasMention={ch.hasMention}
              active={currentChannel === ch.name}
              onClick={() => onChannelSelect(ch.name)}
            />
          ))}
        </ul>
      </div>

      {/* Create channel dialog */}
      <Dialog open={createOpen} onOpenChange={(open) => { setCreateOpen(open); if (!open) resetCreateForm(); }}>
        <DialogContent className="sm:max-w-md">
          <DialogHeader>
            <DialogTitle>Create Channel</DialogTitle>
          </DialogHeader>
          <form
            onSubmit={(e) => { e.preventDefault(); handleCreateChannel(); }}
            className="grid gap-3"
          >
            <div className="grid gap-1.5">
              <label htmlFor="ch-name" className="text-sm font-medium">Name</label>
              <Input
                id="ch-name"
                placeholder="e.g. design-review"
                value={createName}
                onChange={(e) => setCreateName(e.target.value)}
                autoFocus
              />
              <p className="text-[11px] text-muted-foreground">Lowercase letters, numbers, hyphens. Max 32 chars.</p>
            </div>
            <div className="grid gap-1.5">
              <label htmlFor="ch-display" className="text-sm font-medium">Display Name <span className="text-muted-foreground font-normal">(optional)</span></label>
              <Input
                id="ch-display"
                placeholder="e.g. Design Review"
                value={createDisplayName}
                onChange={(e) => setCreateDisplayName(e.target.value)}
              />
            </div>
            <div className="grid gap-1.5">
              <label htmlFor="ch-intro" className="text-sm font-medium">Introduction <span className="text-muted-foreground font-normal">(optional)</span></label>
              <Input
                id="ch-intro"
                placeholder="What is this channel about?"
                value={createIntro}
                onChange={(e) => setCreateIntro(e.target.value)}
              />
            </div>
            {createError && (
              <p className="text-sm text-destructive">{createError}</p>
            )}
            <DialogFooter>
              <Button type="submit" disabled={creating || !createName.trim()}>
                {creating ? "Creating..." : "Create"}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>

      {/* DMs section */}
      <div className="px-3 pt-3 pb-4">
        <div className="flex items-center justify-between mb-2 px-2">
          <p className="text-[10px] font-semibold uppercase text-muted-foreground tracking-widest">
            Direct Messages
          </p>
          <Popover open={dmSearchOpen} onOpenChange={setDmSearchOpen}>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                size="icon-xs"
                title="New DM"
                className="text-muted-foreground hover:text-foreground"
              >
                <span className="text-base leading-none">+</span>
              </Button>
            </PopoverTrigger>
            <PopoverContent side="right" align="start" className="w-52 p-1">
              <Input
                ref={inputRef}
                placeholder="Search users..."
                value={dmQuery}
                onChange={(e) => setDmQuery(e.target.value)}
                className="h-7 text-xs mb-1"
              />
              {filteredUsers.length > 0 && (
                <ul className="max-h-40 overflow-y-auto">
                  {filteredUsers.map((u) => (
                    <li
                      key={u}
                      className="px-2 py-1.5 text-sm rounded-sm cursor-pointer hover:bg-accent hover:text-accent-foreground transition-colors"
                      onMouseDown={() => handleUserSelect(u)}
                    >
                      @{u}
                    </li>
                  ))}
                </ul>
              )}
              {filteredUsers.length === 0 && dmQuery.trim() && (
                <p className="px-2 py-1.5 text-xs text-muted-foreground">No users found</p>
              )}
            </PopoverContent>
          </Popover>
        </div>

        <ul className="space-y-0.5">
          {dmChannels.map((ch) => {
            const label = `@ ${dmDisplayName(ch, currentUser)}`;
            return (
              <ChannelItem
                key={ch.name}
                label={label}
                unread={ch.unreadCount}
                hasMention={ch.hasMention}
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
  hasMention: boolean;
  active: boolean;
  onClick: () => void;
}

function ChannelItem({ label, unread, hasMention, active, onClick }: ChannelItemProps) {
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
            {hasMention ? `${unread}(@)` : unread}
          </Badge>
        )}
      </button>
    </li>
  );
}
