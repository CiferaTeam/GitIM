import { useEffect, useRef, useState } from "react";
import { Hash, AtSign, ArchiveRestore, ChevronRight, Plus, Search } from "lucide-react";
import { toast } from "sonner";
import { useChatStore } from "../../hooks/use-chat-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
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
import { MemberPicker } from "./member-picker";

interface SidebarProps {
  onChannelSelect: (name: string) => void;
  onStartDm: (targetUser: string) => void;
}

function dmDisplayName(channel: Channel, currentUser: string): string {
  const parts = channel.name.split("--");
  if (parts.length !== 2) return channel.name;
  const [a, b] = parts;
  if (a === b || (a === currentUser && b === currentUser)) {
    return `${currentUser} (me)`;
  }
  if (a === currentUser) return b;
  if (b === currentUser) return a;
  return `${a} ↔ ${b}`;
}

function isSelfDm(channel: Channel, currentUser: string): boolean {
  const parts = channel.name.split("--");
  return parts.length === 2 && parts[0] === currentUser && parts[1] === currentUser;
}

function isMyDm(channel: Channel, currentUser: string): boolean {
  const parts = channel.name.split("--");
  return parts.length === 2 && (parts[0] === currentUser || parts[1] === currentUser);
}

export function Sidebar({ onChannelSelect, onStartDm }: SidebarProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const currentUser = useChatStore((s) => s.currentUser);
  const channels = useChatStore((s) => s.channels);
  const archivedChannels = useChatStore((s) => s.archivedChannels);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const users = useChatStore((s) => s.users);
  const setChannels = useChatStore((s) => s.setChannels);
  const setArchivedChannels = useChatStore((s) => s.setArchivedChannels);
  const markChannelUnarchived = useChatStore((s) => s.markChannelUnarchived);

  const [archivedOpen, setArchivedOpen] = useState(false);
  const [archivedLoaded, setArchivedLoaded] = useState(false);

  const [dmSearchOpen, setDmSearchOpen] = useState(false);
  const [dmQuery, setDmQuery] = useState("");
  const [channelQuery, setChannelQuery] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);

  const [createOpen, setCreateOpen] = useState(false);
  const [createName, setCreateName] = useState("");
  const [createDisplayName, setCreateDisplayName] = useState("");
  const [createIntro, setCreateIntro] = useState("");
  const [createInvitees, setCreateInvitees] = useState<string[]>([]);
  const [createError, setCreateError] = useState("");
  const [creating, setCreating] = useState(false);

  function resetCreateForm() {
    setCreateName("");
    setCreateDisplayName("");
    setCreateIntro("");
    setCreateInvitees([]);
    setCreateError("");
    setCreating(false);
  }

  async function handleCreateChannel() {
    if (!activeSlug) {
      setCreateError("No workspace selected");
      return;
    }
    const name = createName.trim().toLowerCase();
    const validation = client.validateChannelName(name);
    if (validation) {
      setCreateError(validation);
      return;
    }
    setCreating(true);
    setCreateError("");
    try {
      const res = await client.createChannel(
        activeSlug,
        name,
        createDisplayName.trim() || undefined,
        createIntro.trim() || undefined,
        createInvitees.length > 0 ? createInvitees : undefined,
      );
      if (!res.ok) {
        setCreateError(res.error ?? "Failed to create channel");
        setCreating(false);
        return;
      }
    } catch {
      setCreateError("Network error — is the server running?");
      setCreating(false);
      return;
    }
    try {
      const chRes = await client.channels(activeSlug);
      if (chRes.ok && chRes.data) {
        setChannels(chRes.data.channels as Channel[]);
      }
    } catch { /* refresh failure is non-fatal */ }
    resetCreateForm();
    setCreateOpen(false);
    onChannelSelect(name);
  }

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
      const aMy = isMyDm(a, currentUser);
      const bMy = isMyDm(b, currentUser);
      if (aMy && !bMy) return -1;
      if (!aMy && bMy) return 1;
      return a.name.localeCompare(b.name);
    });

  const myDmChannels = dmChannels.filter((c) => isMyDm(c, currentUser));
  const otherDmChannels = dmChannels.filter((c) => !isMyDm(c, currentUser));

  const filteredRegularChannels = channelQuery.trim()
    ? regularChannels.filter((c) =>
        c.name.toLowerCase().includes(channelQuery.toLowerCase())
      )
    : regularChannels;

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

  async function handleToggleArchivedSection() {
    const next = !archivedOpen;
    setArchivedOpen(next);
    // Fetch archived channels the first time the section is opened.
    // Session cache: subsequent opens skip the fetch; unarchive updates the
    // local list directly.
    if (next && !archivedLoaded) {
      if (!activeSlug) return;
      const res = await client.listArchivedChannels(activeSlug);
      if (res.ok && res.data) {
        setArchivedChannels(res.data.channels);
        setArchivedLoaded(true);
      } else {
        toast.error(
          `Failed to load archived channels: ${res.error ?? "unknown"}`,
        );
      }
    }
  }

  async function handleUnarchiveChannel(name: string) {
    if (!activeSlug) return;
    const res = await client.unarchiveChannel(activeSlug, name);
    if (!res.ok) {
      toast.error(`Failed to unarchive #${name}: ${res.error ?? "unknown"}`);
      return;
    }
    markChannelUnarchived(name);
    toast.success(`#${name} restored`);
    // Refresh channel list so the restored channel picks up full metadata
    // (kind, members) in the active `channels` store.
    try {
      const chRes = await client.channels(activeSlug);
      if (chRes.ok && chRes.data) {
        setChannels(chRes.data.channels as Channel[]);
      }
    } catch {
      /* refresh is best-effort; markChannelUnarchived already seeded the entry */
    }
  }

  return (
    <div className="w-64 shrink-0 border-r border-border bg-card/40 flex flex-col overflow-hidden">
      {/* Agent status panel */}
      <AgentStatusPanel />

      {/* Channels section */}
      <div className="px-3 pt-4 pb-2 flex flex-col min-h-0 flex-1 overflow-hidden">
        <div className="flex items-center justify-between mb-2 px-2">
          <p className="text-xs font-semibold uppercase text-text-secondary tracking-wider">
            Channels
          </p>
          <Button
            variant="ghost"
            size="icon-xs"
            title="Create channel"
            className="text-muted-foreground hover:text-foreground"
            onClick={() => setCreateOpen(true)}
          >
            <Plus className="size-3.5" />
          </Button>
        </div>

        {/* Channel search */}
        <div className="relative mb-2 px-1">
          <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-text-faint" />
          <input
            type="text"
            value={channelQuery}
            onChange={(e) => setChannelQuery(e.target.value)}
            placeholder="Search channels..."
            className="w-full h-7 pl-7 pr-2 rounded-md border border-border/60 bg-background/60 text-xs placeholder:text-text-faint focus:outline-none focus:ring-1 focus:ring-ring/50"
          />
        </div>

        <div className="overflow-y-auto -mx-1 px-1 space-y-0.5">
          {filteredRegularChannels.map((ch) => (
            <ChannelItem
              key={ch.name}
              icon={<Hash className="size-3.5 text-text-muted" />}
              label={ch.name}
              unread={ch.unreadCount}
              hasMention={ch.hasMention}
              active={currentChannel === ch.name}
              onClick={() => onChannelSelect(ch.name)}
            />
          ))}
          {filteredRegularChannels.length === 0 && channelQuery.trim() && (
            <p className="px-2 py-1 text-[11px] text-text-muted">No channels found</p>
          )}
        </div>
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
            <div className="grid gap-1.5">
              <label className="text-sm font-medium">Invite members <span className="text-muted-foreground font-normal">(optional)</span></label>
              <MemberPicker
                allUsers={users}
                excludeHandlers={currentUser ? [currentUser] : []}
                value={createInvitees}
                onChange={setCreateInvitees}
                placeholder="Search users to invite..."
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

      {/* Archived channels section — collapsed by default; lazy-loaded on expand. */}
      <div className="px-3 py-2 border-t border-border/60 shrink-0">
        <button
          type="button"
          onClick={handleToggleArchivedSection}
          className="w-full flex items-center gap-1.5 px-2 py-1 rounded-md text-xs text-text-muted hover:text-text-secondary hover:bg-surface/40 transition-colors"
          aria-expanded={archivedOpen}
        >
          <ChevronRight
            className={[
              "size-3 transition-transform duration-150",
              archivedOpen ? "rotate-90" : "",
            ].join(" ")}
          />
          <span className="uppercase font-semibold tracking-wider">Archived</span>
          {archivedChannels.length > 0 && (
            <span className="ml-1 text-text-faint font-mono">
              {archivedChannels.length}
            </span>
          )}
        </button>
        {archivedOpen && (
          <ul className="mt-1 space-y-0.5 max-h-40 overflow-y-auto">
            {archivedChannels.length === 0 ? (
              <li className="px-2 py-1.5 text-[11px] text-text-muted">
                {archivedLoaded ? "No archived channels" : "Loading…"}
              </li>
            ) : (
              archivedChannels.map((ch) => (
                <li
                  key={ch.name}
                  className="flex items-center gap-1 px-2 py-1.5 rounded-md text-xs text-text-muted opacity-70 hover:opacity-100 hover:bg-surface/40 transition-all group"
                  title="Archived — not selectable. Click the restore button to unarchive."
                >
                  <Hash className="size-3 text-text-faint shrink-0" />
                  <span className="truncate flex-1">{ch.name}</span>
                  <Button
                    variant="ghost"
                    size="icon-xs"
                    title={`Unarchive #${ch.name}`}
                    className="text-text-faint hover:text-foreground opacity-0 group-hover:opacity-100 transition-opacity"
                    onClick={(e) => {
                      e.stopPropagation();
                      handleUnarchiveChannel(ch.name);
                    }}
                  >
                    <ArchiveRestore className="size-3" />
                  </Button>
                </li>
              ))
            )}
          </ul>
        )}
      </div>

      {/* DMs section */}
      <div className="px-3 pt-3 pb-4 border-t border-border/60 flex flex-col min-h-0 max-h-[45%]">
        <div className="flex items-center justify-between mb-2 px-2">
          <p className="text-xs font-semibold uppercase text-text-secondary tracking-wider">
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
                <Plus className="size-3.5" />
              </Button>
            </PopoverTrigger>
            <PopoverContent side="right" align="start" className="w-56 p-2">
              <Input
                ref={inputRef}
                placeholder="Search users..."
                value={dmQuery}
                onChange={(e) => setDmQuery(e.target.value)}
                className="h-8 text-xs mb-1"
              />
              {filteredUsers.length > 0 && (
                <ul className="max-h-40 overflow-y-auto space-y-0.5">
                  {filteredUsers.map((u) => (
                    <li
                      key={u}
                      className="px-2 py-1.5 text-sm rounded-md cursor-pointer hover:bg-accent hover:text-accent-foreground transition-colors"
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

        <div className="overflow-y-auto -mx-1 px-1 space-y-0.5">
          {myDmChannels.map((ch) => {
            const label = dmDisplayName(ch, currentUser);
            return (
              <ChannelItem
                key={ch.name}
                icon={<AtSign className="size-3.5 text-text-muted" />}
                label={label}
                unread={ch.unreadCount}
                hasMention={ch.hasMention}
                active={currentChannel === ch.name}
                onClick={() => onChannelSelect(ch.name)}
              />
            );
          })}
          {otherDmChannels.length > 0 && myDmChannels.length > 0 && (
            <div className="pt-2 pb-0.5 px-2">
              <p className="text-[10px] font-semibold uppercase text-text-faint tracking-wider">
                Others
              </p>
            </div>
          )}
          {otherDmChannels.map((ch) => {
            const label = dmDisplayName(ch, currentUser);
            return (
              <ChannelItem
                key={ch.name}
                icon={<AtSign className="size-3.5 text-text-muted" />}
                label={label}
                unread={ch.unreadCount}
                hasMention={ch.hasMention}
                active={currentChannel === ch.name}
                onClick={() => onChannelSelect(ch.name)}
              />
            );
          })}
        </div>
      </div>
    </div>
  );
}

interface ChannelItemProps {
  icon: React.ReactNode;
  label: string;
  unread: number;
  hasMention: boolean;
  active: boolean;
  onClick: () => void;
}

function ChannelItem({ icon, label, unread, hasMention, active, onClick }: ChannelItemProps) {
  return (
    <li>
      <button
        type="button"
        onClick={onClick}
        className={[
          "w-full flex items-center gap-2 rounded-md px-2.5 py-2 text-sm text-left transition-all duration-150",
          active
            ? "bg-primary/15 text-primary font-medium border-l-2 border-primary"
            : "hover:bg-surface/60 text-text-secondary hover:text-foreground border-l-2 border-transparent",
          unread > 0 && !active ? "text-foreground font-medium" : "",
        ].join(" ")}
      >
        {icon}
        <span className="truncate flex-1">{label}</span>
        {unread > 0 && (
          <Badge
            variant="default"
            className={[
              "ml-1 text-[10px] px-1.5 py-0 h-4 min-w-4 font-mono",
              hasMention ? "bg-primary text-white" : "bg-surface-hover text-foreground border border-border",
            ].join(" ")}
          >
            {hasMention ? `${unread}@` : unread}
          </Badge>
        )}
      </button>
    </li>
  );
}
