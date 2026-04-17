import { useState } from "react";
import { UserPlus, Users } from "lucide-react";
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
      <div className="h-12 border-b border-border/60 flex items-center px-4 shrink-0">
        <span className="text-sm text-muted-foreground">
          Select a channel or DM
        </span>
      </div>
    );
  }

  const channel = channels.find((c) => c.name === currentChannel);
  const isDm = channel?.kind === "dm";

  // Channel display: "#general" or "@alice"
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
  // daemon-side validate_join 只允许 member 邀请他人；非成员场景（有 Join banner）
  // 若仍显示 Invite 入口，点击必失败 → 隐藏更诚实
  const canInvite = !isDm && !!currentUser && members.includes(currentUser);

  return (
    <div className="h-12 border-b border-border/60 flex items-center px-4 justify-between shrink-0">
      {/* Left: back button + channel name */}
      <div className="flex items-center">
        {children}
        <span className="font-semibold text-sm tracking-tight">{displayName}</span>
      </div>

      {/* Right: member list (channels only) */}
      {!isDm && members.length > 0 && (
        <DropdownMenu>
          <DropdownMenuTrigger asChild>
            <Button variant="ghost" size="sm" className="gap-1.5">
              <Users className="size-4" />
              <span>{members.length}</span>
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" className="w-48">
            {canInvite && (
              <>
                <DropdownMenuItem
                  onSelect={() => setInviteOpen(true)}
                  className="gap-2"
                >
                  <UserPlus className="size-3.5" />
                  <span>Invite members</span>
                </DropdownMenuItem>
                <DropdownMenuSeparator />
              </>
            )}
            <DropdownMenuLabel>Members</DropdownMenuLabel>
            <DropdownMenuSeparator />
            {members.map((member) => (
              <DropdownMenuItem
                key={member}
                className="justify-between"
                onSelect={(e) => e.preventDefault()}
              >
                <span>@{member}</span>
                {member !== currentUser && (
                  <Button
                    variant="ghost"
                    size="xs"
                    onClick={() => onStartDm(member)}
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
