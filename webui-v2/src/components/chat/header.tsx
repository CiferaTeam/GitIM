import { useState } from "react";
import { UserPlus, Users, Hash, AtSign } from "lucide-react";
import { useChatStore } from "../../hooks/use-chat-store";
import * as client from "../../lib/client";
import type { Channel } from "../../lib/types";
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
  children?: React.ReactNode;
}

export function ChatHeader({ onStartDm, children }: ChatHeaderProps) {
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const currentUser = useChatStore((s) => s.currentUser);
  const allUsers = useChatStore((s) => s.users);
  const setChannels = useChatStore((s) => s.setChannels);

  const [inviteOpen, setInviteOpen] = useState(false);

  async function refreshChannels() {
    const chRes = await client.channels();
    if (chRes.ok && chRes.data) {
      setChannels(chRes.data.channels as Channel[]);
    }
  }

  if (!currentChannel) {
    return (
      <div className="h-12 border-b border-border flex items-center px-4 shrink-0 bg-card/30">
        <span className="text-sm text-text-muted">
          Select a channel or DM
        </span>
      </div>
    );
  }

  const channel = channels.find((c) => c.name === currentChannel);
  const isDm = channel?.kind === "dm";

  let displayName: string;
  if (isDm) {
    const parts = currentChannel.split("--");
    if (parts.length === 2) {
      const other = parts.find((p) => p !== currentUser) ?? parts[0];
      displayName = `@${other}`;
    } else {
      displayName = `@${currentChannel}`;
    }
  } else {
    displayName = `#${currentChannel}`;
  }

  const members = channel?.members ?? [];
  const canInvite = !isDm && !!currentUser && members.includes(currentUser);

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
          <span className="font-semibold text-sm tracking-tight truncate">{displayName}</span>
        </div>
      </div>

      {/* Right: member list (channels only) */}
      {!isDm && members.length > 0 && (
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
            <DropdownMenuLabel className="text-text-muted text-xs uppercase tracking-wider">Members</DropdownMenuLabel>
            <DropdownMenuSeparator className="bg-border" />
            {members.map((member) => (
              <DropdownMenuItem
                key={member}
                className="justify-between cursor-default focus:bg-transparent"
                onSelect={(e) => e.preventDefault()}
              >
                <span className="text-sm">@{member}</span>
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
          </DropdownMenuContent>
        </DropdownMenu>
      )}

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
