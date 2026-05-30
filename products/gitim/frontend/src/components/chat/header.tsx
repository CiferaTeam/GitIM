import { useMemo, useState } from "react";
import { Archive, AtSign, Crown, Hash, LayoutGrid, UserPlus, Users } from "lucide-react";
import { toast } from "sonner";
import { useCardStore } from "../../hooks/use-card-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useWorkspaceStore } from "../../hooks/use-workspace-store";
import * as client from "../../lib/client";
import { dmPeerHandler, formatDmDisplayName } from "../../lib/dm-display-name";
import type { Channel } from "../../lib/types";
import { HandlerName } from "./handler-name";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";
import { InviteDialog } from "./invite-dialog";

interface ChatHeaderProps {
  onStartDm: (targetUser: string) => void;
  onOpenCards?: () => void;
  children?: React.ReactNode;
}

export function ChatHeader({ onStartDm, onOpenCards, children }: ChatHeaderProps) {
  const activeSlug = useWorkspaceStore((s) => s.activeSlug);
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const currentUser = useChatStore((s) => s.currentUser);
  const allUsers = useChatStore((s) => s.users);
  const setChannels = useChatStore((s) => s.setChannels);
  const markChannelArchived = useChatStore((s) => s.markChannelArchived);
  const selectChannel = useChatStore((s) => s.selectChannel);
  const cards = useCardStore((s) => s.cards);

  const [archiving, setArchiving] = useState(false);

  const [inviteOpen, setInviteOpen] = useState(false);

  const channel = channels.find((c) => c.name === currentChannel);
  const isDm = channel?.kind === "dm";

  // Count cards in the current channel (channel scope only — DMs don't have cards)
  const cardCount = useMemo(
    () =>
      !currentChannel || isDm
        ? 0
        : cards.filter((c) => c.channel === currentChannel).length,
    [cards, currentChannel, isDm],
  );

  async function refreshChannels() {
    if (!activeSlug) return;
    const chRes = await client.channels(activeSlug);
    if (chRes.ok && chRes.data) {
      setChannels(chRes.data.channels as Channel[]);
    }
  }

  async function handleArchive() {
    if (!activeSlug || !currentChannel || isDm || archiving) return;
    if (channel?.created_by?.trim() !== currentUser) return;
    const ok = window.confirm(
      `Archive #${currentChannel}? Members will lose access. You can restore it later from the sidebar's "Archived" section.`,
    );
    if (!ok) return;
    setArchiving(true);
    try {
      const res = await client.archiveChannel(activeSlug, currentChannel);
      if (!res.ok) {
        toast.error(`Failed to archive #${currentChannel}: ${res.error ?? "unknown"}`);
        return;
      }
      const archived = currentChannel;
      const fallback = channels.find(
        (c) => c.name !== archived && c.kind === "channel",
      );
      markChannelArchived(archived);
      selectChannel(fallback?.name ?? null);
      toast.success(`#${archived} archived`);
    } finally {
      setArchiving(false);
    }
  }

  if (!currentChannel) {
    return (
      <div className="h-12 border-b border-border flex items-center px-4 shrink-0 bg-card/30">
        <span className="text-sm text-text-muted">Select a channel or DM</span>
      </div>
    );
  }

  const displayName = isDm
    ? formatDmDisplayName(currentChannel, currentUser)
    : currentChannel;
  const dmPeer = isDm ? dmPeerHandler(currentChannel, currentUser) : null;

  const members = channel?.members ?? [];
  const canInvite = !isDm && !!currentUser && members.includes(currentUser);
  const creator = !isDm ? channel?.created_by?.trim() : null;
  const showChannelInfo = !isDm && (members.length > 0 || !!creator);

  return (
    <div className="h-12 border-b border-border flex items-center px-4 justify-between shrink-0 bg-card/30">
      {/* Left: back button + channel name */}
      <div className="flex items-center gap-2 min-w-0">
        {children}
        <div className="flex items-center gap-1.5">
          {isDm ? (
            <AtSign className="size-4 text-primary shrink-0" />
          ) : (
            <Hash className="size-4 text-primary shrink-0" />
          )}
          <span className="font-semibold text-sm tracking-tight truncate">
            {dmPeer ? <HandlerName handler={dmPeer} /> : displayName}
          </span>
        </div>
      </div>

      {/* Right: cards drawer + member list (channels only) */}
      <div className="flex items-center gap-1">
        {!isDm && onOpenCards && (
          <Button
            variant="ghost"
            size="sm"
            onClick={onOpenCards}
            className="gap-1.5 text-text-secondary hover:text-foreground hover:bg-surface-hover"
            title={`Cards in #${currentChannel}`}
          >
            <LayoutGrid className="size-4" />
            <span>Cards</span>
            {cardCount > 0 && (
              <span className="text-[10px] rounded-full bg-primary/20 text-primary px-1.5 py-0.5 leading-none">
                {cardCount}
              </span>
            )}
          </Button>
        )}
        {showChannelInfo && (
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button variant="ghost" size="sm" className="gap-1.5 text-text-secondary hover:text-foreground hover:bg-surface-hover">
                <Users className="size-4" />
                <span>{members.length}</span>
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="w-52 bg-popover border-border">
              {canInvite && (
                <>
                  <DropdownMenuItem
                    onSelect={() => setInviteOpen(true)}
                    className="gap-2 cursor-pointer focus:bg-surface-hover"
                  >
                    <UserPlus className="size-3.5" />
                    <span>Invite members</span>
                  </DropdownMenuItem>
                  <DropdownMenuSeparator className="bg-border" />
                </>
              )}
              {creator && (
                <>
                  <DropdownMenuLabel className="text-text-muted text-xs uppercase tracking-wider">Creator</DropdownMenuLabel>
                  <DropdownMenuItem
                    className="justify-between cursor-default focus:bg-transparent"
                    onSelect={(e) => e.preventDefault()}
                  >
                    <span className="flex items-center gap-1.5 text-sm text-foreground">
                      <Crown className="size-3.5 text-warning" />
                      <HandlerName handler={creator} />
                    </span>
                    {creator !== currentUser && (
                      <Button
                        variant="ghost"
                        size="xs"
                        onClick={() => onStartDm(creator)}
                        className="text-primary hover:text-primary hover:bg-primary/10"
                      >
                        DM
                      </Button>
                    )}
                  </DropdownMenuItem>
                  <DropdownMenuSeparator className="bg-border" />
                </>
              )}
              {members.length > 0 && (
                <>
                  <DropdownMenuLabel className="text-text-muted text-xs uppercase tracking-wider">Members</DropdownMenuLabel>
                  <DropdownMenuSeparator className="bg-border" />
                  {members.map((member) => (
                    <DropdownMenuItem
                      key={member}
                      className="justify-between cursor-default focus:bg-transparent"
                      onSelect={(e) => e.preventDefault()}
                    >
                      <span className="text-sm">
                        <HandlerName handler={member} />
                      </span>
                      {member !== currentUser && (
                        <Button
                          variant="ghost"
                          size="xs"
                          onClick={() => onStartDm(member)}
                          className="text-primary hover:text-primary hover:bg-primary/10"
                        >
                          DM
                        </Button>
                      )}
                    </DropdownMenuItem>
                  ))}
                </>
              )}
              {!isDm && creator === currentUser && (
                <>
                  <DropdownMenuSeparator className="bg-border" />
                  <DropdownMenuItem
                    onSelect={handleArchive}
                    disabled={archiving}
                    className="gap-2 cursor-pointer text-destructive focus:bg-destructive/10 focus:text-destructive"
                  >
                    <Archive className="size-3.5" />
                    <span>Archive channel</span>
                  </DropdownMenuItem>
                </>
              )}
            </DropdownMenuContent>
          </DropdownMenu>
        )}
      </div>

      {canInvite && (
        <InviteDialog
          open={inviteOpen}
          onOpenChange={setInviteOpen}
          channel={currentChannel}
          allUsers={allUsers}
          excludeHandlers={[currentUser, ...members].filter(Boolean)}
          onInvited={refreshChannels}
        />
      )}
    </div>
  );
}
