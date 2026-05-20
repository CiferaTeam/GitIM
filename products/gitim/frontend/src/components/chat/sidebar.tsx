import { useEffect, useMemo, useRef, useState } from "react";
import { Hash, AtSign, Archive, ArchiveRestore, ChevronRight, Pin, Plus, Search } from "lucide-react";
import { toast } from "sonner";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useConnectionStore } from "../../hooks/use-connection-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import * as client from "../../lib/client";
import type { Channel } from "../../lib/types";
import { workspaceIdentity } from "../../lib/workspace-key";
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

const KNOWN_AGENT_STORAGE_PREFIX = "gitim-known-agents:";
const PINNED_CONVERSATIONS_STORAGE_PREFIX = "gitim-pinned-conversations:";

// Archived DMs are loaded one page at a time. 5 is a sidebar-friendly batch
// — small enough that the section doesn't dwarf the active DM list, large
// enough that a typical user can find what they want in 1–2 pages.
const ARCHIVED_DMS_PAGE_SIZE = 5;
// Debounce window between the user typing in the prefix filter and the
// fetch firing. 300ms is the standard "feels responsive without flooding".
const ARCHIVED_DMS_PREFIX_DEBOUNCE_MS = 300;
const ARCHIVED_CHANNELS_PAGE_SIZE = 10;
const ARCHIVED_CHANNELS_PREFIX_DEBOUNCE_MS = 300;

interface PinnedConversations {
  channels: Set<string>;
  dms: Set<string>;
}

function knownAgentStorageKey(workspaceKey: string): string {
  return `${KNOWN_AGENT_STORAGE_PREFIX}${workspaceKey}`;
}

function pinnedConversationsStorageKey(workspaceKey: string): string {
  return `${PINNED_CONVERSATIONS_STORAGE_PREFIX}${workspaceKey}`;
}

function readKnownAgentIds(workspaceKey: string | null): Set<string> {
  if (!workspaceKey) return new Set();
  try {
    const raw = localStorage.getItem(knownAgentStorageKey(workspaceKey));
    const parsed = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(parsed) ? parsed.filter((v) => typeof v === "string") : []);
  } catch {
    return new Set();
  }
}

function writeKnownAgentIds(workspaceKey: string, ids: Set<string>) {
  localStorage.setItem(knownAgentStorageKey(workspaceKey), JSON.stringify([...ids].sort()));
}

function emptyPinnedConversations(): PinnedConversations {
  return { channels: new Set(), dms: new Set() };
}

function readPinnedConversations(workspaceKey: string | null): PinnedConversations {
  if (!workspaceKey) return emptyPinnedConversations();
  try {
    const raw = localStorage.getItem(pinnedConversationsStorageKey(workspaceKey));
    const parsed = raw ? (JSON.parse(raw) as unknown) : {};
    const record =
      parsed && typeof parsed === "object"
        ? (parsed as { channels?: unknown; dms?: unknown })
        : {};
    const channels = Array.isArray(record.channels) ? record.channels : [];
    const dms = Array.isArray(record.dms) ? record.dms : [];
    return {
      channels: new Set(channels.filter((v): v is string => typeof v === "string")),
      dms: new Set(dms.filter((v): v is string => typeof v === "string")),
    };
  } catch {
    return emptyPinnedConversations();
  }
}

function writePinnedConversations(workspaceKey: string, pins: PinnedConversations) {
  localStorage.setItem(
    pinnedConversationsStorageKey(workspaceKey),
    JSON.stringify({
      channels: [...pins.channels].sort(),
      dms: [...pins.dms].sort(),
    }),
  );
}

function clonePinnedConversations(pins: PinnedConversations): PinnedConversations {
  return {
    channels: new Set(pins.channels),
    dms: new Set(pins.dms),
  };
}

function equalStringSets(a: Set<string>, b: Set<string>): boolean {
  if (a.size !== b.size) return false;
  for (const value of a) {
    if (!b.has(value)) return false;
  }
  return true;
}

function sortUnreadThenPinned(
  items: Channel[],
  pinnedNames: Set<string>,
): Channel[] {
  return [...items].sort((a, b) => {
    const aUnread = (a.unreadCount || 0) > 0;
    const bUnread = (b.unreadCount || 0) > 0;
    if (aUnread !== bUnread) return aUnread ? -1 : 1;

    const aMention = aUnread && a.hasMention;
    const bMention = bUnread && b.hasMention;
    if (aMention !== bMention) return aMention ? -1 : 1;

    const aPinned = pinnedNames.has(a.name);
    const bPinned = pinnedNames.has(b.name);
    if (aPinned === bPinned) return 0;
    return aPinned ? -1 : 1;
  });
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

function isMyDm(channel: Channel, currentUser: string): boolean {
  const parts = channel.name.split("--");
  return parts.length === 2 && (parts[0] === currentUser || parts[1] === currentUser);
}

function shouldHideDmChannel(
  channel: Channel,
  currentUser: string,
  liveAgentIds: Set<string>,
  knownAgentIds: Set<string>,
): boolean {
  if (channel.kind !== "dm") return false;
  const parts = channel.name.split("--");
  if (parts.length !== 2) return false;
  return parts.some(
    (handler) =>
      handler !== currentUser &&
      knownAgentIds.has(handler) &&
      !liveAgentIds.has(handler),
  );
}

export function Sidebar({ onChannelSelect, onStartDm }: SidebarProps) {
  const mode = useConnectionStore((s) => s.mode);
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const workspaces = useWorkspaceStore((s) => s.workspaces);
  const agents = useAgentStore((s) => s.agents);
  const currentUser = useChatStore((s) => s.currentUser);
  const channels = useChatStore((s) => s.channels);
  const archivedChannels = useChatStore((s) => s.archivedChannels);
  const archivedChannelsView = useChatStore((s) => s.archivedChannelsView);
  const resetArchivedChannelsView = useChatStore(
    (s) => s.resetArchivedChannelsView,
  );
  const appendArchivedChannelsPage = useChatStore(
    (s) => s.appendArchivedChannelsPage,
  );
  const setArchivedChannelsLoading = useChatStore(
    (s) => s.setArchivedChannelsLoading,
  );
  const setArchivedChannelsError = useChatStore(
    (s) => s.setArchivedChannelsError,
  );
  // Pull each field of `archivedDmsView` individually — returning the
  // object itself works, but selecting fields keeps re-render scope tight
  // (and matches the project's selector style; see memory note on
  // `project_zustand_selector_pitfalls.md`).
  const archivedDmsView = useChatStore((s) => s.archivedDmsView);
  const resetArchivedDmsView = useChatStore((s) => s.resetArchivedDmsView);
  const appendArchivedDmsPage = useChatStore(
    (s) => s.appendArchivedDmsPage,
  );
  const setArchivedDmsLoading = useChatStore(
    (s) => s.setArchivedDmsLoading,
  );
  const setArchivedDmsError = useChatStore((s) => s.setArchivedDmsError);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const users = useChatStore((s) => s.users);
  const setChannels = useChatStore((s) => s.setChannels);
  const markChannelUnarchived = useChatStore((s) => s.markChannelUnarchived);
  const markDmArchived = useChatStore((s) => s.markDmArchived);
  const markDmUnarchived = useChatStore((s) => s.markDmUnarchived);
  const activeWorkspace = activeSlug
    ? workspaces.find((workspace) => workspace.slug === activeSlug)
    : undefined;
  const activeWorkspaceKey = activeWorkspace
    ? workspaceIdentity(mode, activeWorkspace)
    : null;

  const [archivedOpen, setArchivedOpen] = useState(false);
  const [pendingArchivedChannelQuery, setPendingArchivedChannelQuery] =
    useState("");
  const archivedChannelQueryDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const [archivedDmsOpen, setArchivedDmsOpen] = useState(false);
  // The input value the user is *currently typing*. We debounce before
  // pushing it into the store so each keystroke doesn't trigger a fetch.
  const [pendingDmQuery, setPendingDmQuery] = useState("");
  const dmQueryDebounceRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  );
  const [knownAgentIds, setKnownAgentIds] = useState<Set<string>>(
    () => readKnownAgentIds(activeWorkspaceKey),
  );
  const [pinnedConversations, setPinnedConversations] = useState<PinnedConversations>(
    () => readPinnedConversations(activeWorkspaceKey),
  );

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

  useEffect(() => {
    if (!activeWorkspaceKey) {
      setKnownAgentIds(new Set());
      return;
    }
    const next = readKnownAgentIds(activeWorkspaceKey);
    for (const agent of agents) {
      next.add(agent.id);
    }
    writeKnownAgentIds(activeWorkspaceKey, next);
    if (!equalStringSets(knownAgentIds, next)) {
      setKnownAgentIds(next);
    }
  }, [activeWorkspaceKey, agents, knownAgentIds]);

  useEffect(() => {
    setPinnedConversations(readPinnedConversations(activeWorkspaceKey));
  }, [activeWorkspaceKey]);

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

  const regularChannels = sortUnreadThenPinned(
    channels.filter((c) => c.kind === "channel"),
    pinnedConversations.channels,
  );
  const liveAgentIds = useMemo(
    () => new Set(agents.map((agent) => agent.id)),
    [agents],
  );
  const dmChannels = sortUnreadThenPinned(
    channels
      .filter((c) => c.kind === "dm")
      .filter((c) => !shouldHideDmChannel(c, currentUser, liveAgentIds, knownAgentIds))
      .sort((a, b) => {
        const aMy = isMyDm(a, currentUser);
        const bMy = isMyDm(b, currentUser);
        if (aMy && !bMy) return -1;
        if (!aMy && bMy) return 1;
        return a.name.localeCompare(b.name);
      }),
    pinnedConversations.dms,
  );

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
  const archivedChannelItems =
    archivedChannelsView?.items ?? archivedChannels;

  useEffect(() => {
    if (!currentChannel) return;
    const activeChannel = channels.find((c) => c.name === currentChannel);
    if (
      !activeChannel ||
      !shouldHideDmChannel(activeChannel, currentUser, liveAgentIds, knownAgentIds)
    ) {
      return;
    }
    const fallback =
      channels.find((c) => c.kind === "channel" && c.name === "general") ??
      channels.find((c) => c.kind === "channel") ??
      channels.find(
        (c) =>
          c.kind === "dm" &&
          !shouldHideDmChannel(c, currentUser, liveAgentIds, knownAgentIds),
      );
    if (fallback) {
      onChannelSelect(fallback.name);
    }
  }, [currentChannel, channels, currentUser, liveAgentIds, knownAgentIds, onChannelSelect]);

  function handleUserSelect(user: string) {
    setDmSearchOpen(false);
    onStartDm(user);
  }

  function handleTogglePinnedConversation(channel: Channel) {
    if (!activeWorkspaceKey) return;
    setPinnedConversations((prev) => {
      const next = clonePinnedConversations(prev);
      const ids = channel.kind === "dm" ? next.dms : next.channels;
      if (ids.has(channel.name)) {
        ids.delete(channel.name);
      } else {
        ids.add(channel.name);
      }
      writePinnedConversations(activeWorkspaceKey, next);
      return next;
    });
  }

  async function fetchArchivedChannelsPage(query: string, offset: number) {
    if (!activeSlug) return;
    if (offset === 0) {
      resetArchivedChannelsView(query);
    }
    setArchivedChannelsLoading(true);
    try {
      const res = await client.listArchivedChannels(activeSlug, {
        prefix: query,
        offset,
        limit: ARCHIVED_CHANNELS_PAGE_SIZE,
      });
      const liveQuery =
        useChatStore.getState().archivedChannelsView?.query;
      if (liveQuery !== query) return;
      if (res.ok && res.data) {
        appendArchivedChannelsPage({
          items: res.data.channels,
          hasMore: res.data.hasMore,
        });
        setArchivedChannelsLoading(false);
      } else {
        const message = res.error ?? "unknown";
        setArchivedChannelsError(message);
        toast.error(`Failed to load archived channels: ${message}`);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : "network error";
      setArchivedChannelsError(message);
      toast.error(`Failed to load archived channels: ${message}`);
    }
  }

  async function handleToggleArchivedSection() {
    const next = !archivedOpen;
    setArchivedOpen(next);
    if (next && archivedChannelsView === null) {
      await fetchArchivedChannelsPage(pendingArchivedChannelQuery, 0);
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

  useEffect(() => {
    if (
      archivedOpen &&
      archivedChannelsView === null &&
      activeSlug &&
      !archivedChannelQueryDebounceRef.current
    ) {
      void fetchArchivedChannelsPage(pendingArchivedChannelQuery, 0);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archivedOpen, archivedChannelsView, activeSlug]);

  function handleArchivedChannelPrefixChange(value: string) {
    setPendingArchivedChannelQuery(value);
    if (archivedChannelQueryDebounceRef.current) {
      clearTimeout(archivedChannelQueryDebounceRef.current);
    }
    archivedChannelQueryDebounceRef.current = setTimeout(() => {
      archivedChannelQueryDebounceRef.current = null;
      void fetchArchivedChannelsPage(value, 0);
    }, ARCHIVED_CHANNELS_PREFIX_DEBOUNCE_MS);
  }

  function peerFromDmName(name: string): string | null {
    const parts = name.split("--");
    if (parts.length !== 2) return null;
    if (parts[0] === currentUser) return parts[1];
    if (parts[1] === currentUser) return parts[0];
    return null;
  }

  // Fetches a page of archived DMs and writes it into the store. Handles
  // the reset-vs-append decision (offset === 0 means "first page for this
  // query") and drops the response if the user changed the prefix while
  // the request was in flight — the store's `query` field is the source
  // of truth for "what the user is asking for right now".
  async function fetchArchivedDmsPage(query: string, offset: number) {
    if (!activeSlug) return;
    if (offset === 0) {
      resetArchivedDmsView(query);
    }
    setArchivedDmsLoading(true);
    try {
      const res = await client.listArchivedDms(activeSlug, {
        prefix: query,
        offset,
        limit: ARCHIVED_DMS_PAGE_SIZE,
      });
      // Race guard: if the user typed again during the fetch, the store's
      // applied query has moved on. Drop this stale response without touching
      // state — the newer fetch will refill the view.
      const liveQuery =
        useChatStore.getState().archivedDmsView?.query;
      if (liveQuery !== query) return;
      if (res.ok && res.data) {
        appendArchivedDmsPage({
          items: res.data.dms,
          hasMore: res.data.hasMore,
        });
        setArchivedDmsLoading(false);
      } else {
        const message = res.error ?? "unknown";
        setArchivedDmsError(message);
        toast.error(`Failed to load archived DMs: ${message}`);
      }
    } catch (err) {
      const message = err instanceof Error ? err.message : "network error";
      setArchivedDmsError(message);
      toast.error(`Failed to load archived DMs: ${message}`);
    }
  }

  async function handleToggleArchivedDmsSection() {
    const next = !archivedDmsOpen;
    setArchivedDmsOpen(next);
    // First-time expand: fetch page 1 with whatever prefix is currently
    // staged (typically empty). Subsequent expands keep whatever was
    // already loaded — markDmArchived invalidation flips the view back to
    // null, so the next expand after archive-from-elsewhere is a fresh
    // refetch automatically.
    if (next && archivedDmsView === null) {
      await fetchArchivedDmsPage(pendingDmQuery, 0);
    }
  }

  // Auto-refetch when an in-place invalidation (e.g. `markDmArchived` from
  // SSE or an active-DM archive) drops the view back to null while the
  // section is open. Without this, the section would render "Loading…"
  // forever — the user would have to manually collapse / re-expand.
  useEffect(() => {
    if (
      archivedDmsOpen &&
      archivedDmsView === null &&
      activeSlug &&
      !dmQueryDebounceRef.current
    ) {
      void fetchArchivedDmsPage(pendingDmQuery, 0);
    }
    // `fetchArchivedDmsPage` is a closure over store actions / activeSlug
    // — none of those change identity per render in a way that would
    // re-fire this effect spuriously, but listing them keeps the lint rule
    // honest.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [archivedDmsOpen, archivedDmsView, activeSlug]);

  function handleDmPrefixChange(value: string) {
    setPendingDmQuery(value);
    if (dmQueryDebounceRef.current) {
      clearTimeout(dmQueryDebounceRef.current);
    }
    dmQueryDebounceRef.current = setTimeout(() => {
      dmQueryDebounceRef.current = null;
      void fetchArchivedDmsPage(value, 0);
    }, ARCHIVED_DMS_PREFIX_DEBOUNCE_MS);
  }

  // Clean up any pending debounced fetch on unmount so a late-firing
  // setTimeout doesn't write to an unmounted store consumer.
  useEffect(() => {
    return () => {
      if (archivedChannelQueryDebounceRef.current) {
        clearTimeout(archivedChannelQueryDebounceRef.current);
        archivedChannelQueryDebounceRef.current = null;
      }
      if (dmQueryDebounceRef.current) {
        clearTimeout(dmQueryDebounceRef.current);
        dmQueryDebounceRef.current = null;
      }
    };
  }, []);

  async function handleArchiveDm(dmName: string) {
    if (!activeSlug) return;
    const peer = peerFromDmName(dmName);
    if (!peer) {
      toast.error(`Cannot archive DM: not a participant in ${dmName}`);
      return;
    }
    const res = await client.archiveDm(activeSlug, peer);
    if (!res.ok) {
      toast.error(`Failed to archive DM with @${peer}: ${res.error ?? "unknown"}`);
      return;
    }
    markDmArchived(dmName);
    toast.success(`DM with @${peer} archived`);
  }

  async function handleUnarchiveDm(dmName: string) {
    if (!activeSlug) return;
    const peer = peerFromDmName(dmName);
    if (!peer) {
      toast.error(`Cannot unarchive DM: not a participant in ${dmName}`);
      return;
    }
    const res = await client.unarchiveDm(activeSlug, peer);
    if (!res.ok) {
      toast.error(`Failed to unarchive DM with @${peer}: ${res.error ?? "unknown"}`);
      return;
    }
    markDmUnarchived(dmName);
    toast.success(`DM with @${peer} restored`);
    // Refresh channels list so the restored DM picks up authoritative metadata
    // (members) the same way channel unarchive does.
    try {
      const chRes = await client.channels(activeSlug);
      if (chRes.ok && chRes.data) {
        setChannels(chRes.data.channels as Channel[]);
      }
    } catch {
      /* best-effort refresh */
    }
  }

  return (
    <div className="w-64 shrink-0 border-r border-border bg-card/40 flex flex-col overflow-hidden h-full">
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

        <div className="min-h-0 flex-1 overflow-y-auto -mx-1 px-1 space-y-0.5">
          {filteredRegularChannels.map((ch) => (
            <ChannelItem
              key={ch.name}
              icon={<Hash className="size-3.5 text-text-muted" />}
              label={ch.name}
              unread={ch.unreadCount}
              hasMention={ch.hasMention}
              active={currentChannel === ch.name}
              pinned={pinnedConversations.channels.has(ch.name)}
              pinLabel={`Pin #${ch.name}`}
              unpinLabel={`Unpin #${ch.name}`}
              testId="sidebar-channel-item"
              onClick={() => onChannelSelect(ch.name)}
              onTogglePin={() => handleTogglePinnedConversation(ch)}
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
          {archivedChannelItems.length > 0 && (
            <span className="ml-1 text-text-faint font-mono">
              {archivedChannelItems.length}
              {archivedChannelsView?.hasMore ? "+" : ""}
            </span>
          )}
        </button>
        {archivedOpen && (
          <div className="mt-1 space-y-1">
            <div className="relative px-1">
              <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-text-faint" />
              <input
                type="text"
                value={pendingArchivedChannelQuery}
                onChange={(e) =>
                  handleArchivedChannelPrefixChange(e.target.value)
                }
                placeholder="Filter channels..."
                data-testid="sidebar-archived-channel-filter"
                className="w-full h-7 pl-7 pr-2 rounded-md border border-border/60 bg-background/60 text-xs placeholder:text-text-faint focus:outline-none focus:ring-1 focus:ring-ring/50"
              />
            </div>
            <ul className="space-y-0.5 max-h-40 overflow-y-auto">
              {(() => {
                if (archivedChannelsView === null) {
                  return (
                    <li className="px-2 py-1.5 text-[11px] text-text-muted">
                      Loading…
                    </li>
                  );
                }
                if (archivedChannelsView.error) {
                  return (
                    <li className="flex items-center justify-between px-2 py-1.5 text-[11px] text-destructive">
                      <span className="truncate">
                        Failed: {archivedChannelsView.error}
                      </span>
                      <Button
                        variant="ghost"
                        size="icon-xs"
                        title="Retry"
                        onClick={() =>
                          fetchArchivedChannelsPage(
                            archivedChannelsView.query,
                            0,
                          )
                        }
                      >
                        <span className="text-[11px]">Retry</span>
                      </Button>
                    </li>
                  );
                }
                if (
                  archivedChannelsView.loading &&
                  archivedChannelsView.items.length === 0
                ) {
                  return (
                    <li className="px-2 py-1.5 text-[11px] text-text-muted">
                      Loading…
                    </li>
                  );
                }
                if (archivedChannelsView.items.length === 0) {
                  return (
                    <li className="px-2 py-1.5 text-[11px] text-text-muted">
                      {archivedChannelsView.query
                        ? "No matches"
                        : "No archived channels"}
                    </li>
                  );
                }
                return archivedChannelsView.items.map((ch) => {
                  const isActive = currentChannel === ch.name;
                  return (
                    <li
                      key={ch.name}
                      data-testid="sidebar-archived-channel-item"
                      className={[
                        "flex items-center gap-1 px-2 py-1.5 rounded-md text-xs cursor-pointer transition-all group",
                        isActive
                          ? "bg-surface/60 text-foreground opacity-100"
                          : "text-text-muted opacity-70 hover:opacity-100 hover:bg-surface/40",
                      ].join(" ")}
                      title="Archived — read only. Click to view; use the restore button to unarchive."
                      onClick={() => onChannelSelect(ch.name)}
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
                  );
                });
              })()}
            </ul>
            {archivedChannelsView &&
              archivedChannelsView.hasMore &&
              archivedChannelsView.items.length > 0 && (
                <Button
                  variant="ghost"
                  size="xs"
                  className="w-full justify-center text-[11px] text-text-muted hover:text-foreground"
                  disabled={archivedChannelsView.loading}
                  onClick={() =>
                    fetchArchivedChannelsPage(
                      archivedChannelsView.query,
                      archivedChannelsView.offset,
                    )
                  }
                  data-testid="sidebar-archived-channel-load-more"
                >
                  {archivedChannelsView.loading ? "Loading…" : "Load more"}
                </Button>
              )}
          </div>
        )}
      </div>

      {/* DMs section */}
      <div className="px-3 pt-3 pb-4 border-t border-border/60 flex flex-col min-h-0 max-h-[45%] overflow-hidden">
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

        <div className="min-h-0 flex-1 overflow-y-auto -mx-1 px-1 space-y-0.5">
          {myDmChannels.map((ch) => {
            const label = dmDisplayName(ch, currentUser);
            const peer = peerFromDmName(ch.name);
            return (
              <ChannelItem
                key={ch.name}
                icon={<AtSign className="size-3.5 text-text-muted" />}
                label={label}
                unread={ch.unreadCount}
                hasMention={ch.hasMention}
                active={currentChannel === ch.name}
                pinned={pinnedConversations.dms.has(ch.name)}
                pinLabel={`Pin DM ${label}`}
                unpinLabel={`Unpin DM ${label}`}
                testId="sidebar-dm-item"
                onClick={() => onChannelSelect(ch.name)}
                onTogglePin={() => handleTogglePinnedConversation(ch)}
                archiveLabel={peer ? `Archive DM with @${peer}` : undefined}
                onArchive={peer ? () => handleArchiveDm(ch.name) : undefined}
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
            const peer = peerFromDmName(ch.name);
            return (
              <ChannelItem
                key={ch.name}
                icon={<AtSign className="size-3.5 text-text-muted" />}
                label={label}
                unread={ch.unreadCount}
                hasMention={ch.hasMention}
                active={currentChannel === ch.name}
                pinned={pinnedConversations.dms.has(ch.name)}
                pinLabel={`Pin DM ${label}`}
                unpinLabel={`Unpin DM ${label}`}
                testId="sidebar-dm-item"
                onClick={() => onChannelSelect(ch.name)}
                onTogglePin={() => handleTogglePinnedConversation(ch)}
                archiveLabel={peer ? `Archive DM with @${peer}` : undefined}
                onArchive={peer ? () => handleArchiveDm(ch.name) : undefined}
              />
            );
          })}
        </div>

        {/* Archived DMs section — collapsed by default; lazy + paginated +
            prefix-filterable. The view is server-driven: each expand /
            keystroke / Load-more triggers a fresh `client.listArchivedDms`
            call rather than walking a fully-loaded in-memory list. */}
        <div className="pt-2 mt-2 border-t border-border/60 shrink-0">
          <button
            type="button"
            onClick={handleToggleArchivedDmsSection}
            className="w-full flex items-center gap-1.5 px-2 py-1 rounded-md text-xs text-text-muted hover:text-text-secondary hover:bg-surface/40 transition-colors"
            aria-expanded={archivedDmsOpen}
          >
            <ChevronRight
              className={[
                "size-3 transition-transform duration-150",
                archivedDmsOpen ? "rotate-90" : "",
              ].join(" ")}
            />
            <span className="uppercase font-semibold tracking-wider">Archived DMs</span>
            {archivedDmsView && archivedDmsView.items.length > 0 && (
              <span className="ml-1 text-text-faint font-mono">
                {archivedDmsView.items.length}
                {archivedDmsView.hasMore ? "+" : ""}
              </span>
            )}
          </button>
          {archivedDmsOpen && (
            <div className="mt-1 space-y-1">
              {/* Prefix filter — debounced server-side search by peer
                  handle. Empty input = full archive (paginated). */}
              <div className="relative px-1">
                <Search className="absolute left-2.5 top-1/2 -translate-y-1/2 size-3.5 text-text-faint" />
                <input
                  type="text"
                  value={pendingDmQuery}
                  onChange={(e) => handleDmPrefixChange(e.target.value)}
                  placeholder="Filter by handle..."
                  data-testid="sidebar-archived-dm-filter"
                  className="w-full h-7 pl-7 pr-2 rounded-md border border-border/60 bg-background/60 text-xs placeholder:text-text-faint focus:outline-none focus:ring-1 focus:ring-ring/50"
                />
              </div>
              <ul className="space-y-0.5 max-h-40 overflow-y-auto">
                {(() => {
                  // Initial expand: view is null until the first response
                  // lands. Show a Loading placeholder so the section doesn't
                  // appear empty during the round-trip.
                  if (archivedDmsView === null) {
                    return (
                      <li className="px-2 py-1.5 text-[11px] text-text-muted">
                        Loading…
                      </li>
                    );
                  }
                  if (archivedDmsView.error) {
                    return (
                      <li className="flex items-center justify-between px-2 py-1.5 text-[11px] text-destructive">
                        <span className="truncate">
                          Failed: {archivedDmsView.error}
                        </span>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          title="Retry"
                          onClick={() =>
                            fetchArchivedDmsPage(archivedDmsView.query, 0)
                          }
                        >
                          <span className="text-[11px]">Retry</span>
                        </Button>
                      </li>
                    );
                  }
                  if (
                    archivedDmsView.loading &&
                    archivedDmsView.items.length === 0
                  ) {
                    return (
                      <li className="px-2 py-1.5 text-[11px] text-text-muted">
                        Loading…
                      </li>
                    );
                  }
                  if (archivedDmsView.items.length === 0) {
                    return (
                      <li className="px-2 py-1.5 text-[11px] text-text-muted">
                        {archivedDmsView.query
                          ? "No matches"
                          : "No archived DMs"}
                      </li>
                    );
                  }
                  return archivedDmsView.items.map((entry) => {
                    const name = entry.dm_pair_stem;
                    const isActive = currentChannel === name;
                    // The daemon includes `peer` directly, so we don't have to
                    // run `peerFromDmName` against the stem here — saves a
                    // string split and works even when the stored handler is
                    // out-of-sync (e.g. mid-handler-rename) since the daemon
                    // computed `peer` against the request's authenticated user.
                    const label = entry.peer;
                    return (
                      <li
                        key={name}
                        data-testid="sidebar-archived-dm-item"
                        className={[
                          "flex items-center gap-1 px-2 py-1.5 rounded-md text-xs cursor-pointer transition-all group",
                          isActive
                            ? "bg-surface/60 text-foreground opacity-100"
                            : "text-text-muted opacity-70 hover:opacity-100 hover:bg-surface/40",
                        ].join(" ")}
                        title="Archived — read only. Click to view; use the restore button to unarchive."
                        onClick={() => onChannelSelect(name)}
                      >
                        <AtSign className="size-3 text-text-faint shrink-0" />
                        <span className="truncate flex-1">{label}</span>
                        <Button
                          variant="ghost"
                          size="icon-xs"
                          title={`Unarchive DM with @${label}`}
                          className="text-text-faint hover:text-foreground opacity-0 group-hover:opacity-100 transition-opacity"
                          onClick={(e) => {
                            e.stopPropagation();
                            handleUnarchiveDm(name);
                          }}
                        >
                          <ArchiveRestore className="size-3" />
                        </Button>
                      </li>
                    );
                  });
                })()}
              </ul>
              {archivedDmsView &&
                archivedDmsView.hasMore &&
                archivedDmsView.items.length > 0 && (
                  <Button
                    variant="ghost"
                    size="xs"
                    className="w-full justify-center text-[11px] text-text-muted hover:text-foreground"
                    disabled={archivedDmsView.loading}
                    onClick={() =>
                      fetchArchivedDmsPage(
                        archivedDmsView.query,
                        archivedDmsView.offset,
                      )
                    }
                    data-testid="sidebar-archived-dm-load-more"
                  >
                    {archivedDmsView.loading ? "Loading…" : "Load more"}
                  </Button>
                )}
            </div>
          )}
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
  pinned: boolean;
  pinLabel: string;
  unpinLabel: string;
  testId: string;
  onClick: () => void;
  onTogglePin: () => void;
  /** Optional archive action — shows an Archive icon button next to the
   *  pin button on hover. Used by DMs in this revision; channel archive
   *  has its own dedicated UI in the archived section. */
  archiveLabel?: string;
  onArchive?: () => void;
}

function ChannelItem({
  icon,
  label,
  unread,
  hasMention,
  active,
  pinned,
  pinLabel,
  unpinLabel,
  testId,
  onClick,
  onTogglePin,
  archiveLabel,
  onArchive,
}: ChannelItemProps) {
  const pinButtonLabel = pinned ? unpinLabel : pinLabel;
  return (
    <li
      data-testid={testId}
      className={[
        "group flex items-center rounded-md border-l-2 transition-all duration-150",
        active
          ? "bg-primary/15 text-primary font-medium border-primary"
          : "hover:bg-surface/60 text-text-secondary hover:text-foreground border-transparent",
        unread > 0 && !active ? "text-foreground font-medium" : "",
      ].join(" ")}
    >
      <button
        type="button"
        onClick={onClick}
        className="min-w-0 flex-1 flex items-center gap-2 rounded-md px-2.5 py-2 text-sm text-left"
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
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        aria-label={pinButtonLabel}
        aria-pressed={pinned}
        title={pinButtonLabel}
        onClick={(e) => {
          e.stopPropagation();
          onTogglePin();
        }}
        className={[
          "mr-1 text-text-faint transition-opacity hover:text-primary focus-visible:opacity-100",
          pinned ? "opacity-100 text-primary" : "opacity-0 group-hover:opacity-100",
        ].join(" ")}
      >
        <Pin className={["size-3", pinned ? "fill-current" : ""].join(" ")} />
      </Button>
      {onArchive && archiveLabel && (
        <Button
          type="button"
          variant="ghost"
          size="icon-xs"
          aria-label={archiveLabel}
          title={archiveLabel}
          onClick={(e) => {
            e.stopPropagation();
            onArchive();
          }}
          className="mr-1 text-text-faint opacity-0 transition-opacity hover:text-foreground group-hover:opacity-100 focus-visible:opacity-100"
        >
          <Archive className="size-3" />
        </Button>
      )}
    </li>
  );
}
