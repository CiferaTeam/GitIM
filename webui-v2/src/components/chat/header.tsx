import { Users } from "lucide-react";
import { useChatStore } from "../../hooks/use-chat-store";
import { Button } from "../ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "../ui/dropdown-menu";

interface ChatHeaderProps {
  onStartDm: (targetUser: string) => void;
}

export function ChatHeader({ onStartDm }: ChatHeaderProps) {
  const currentChannel = useChatStore((s) => s.currentChannel);
  const channels = useChatStore((s) => s.channels);
  const currentUser = useChatStore((s) => s.currentUser);

  if (!currentChannel) {
    return (
      <div className="h-12 border-b flex items-center px-4 shrink-0">
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

  return (
    <div className="h-12 border-b flex items-center px-4 justify-between shrink-0">
      {/* Left: channel name */}
      <span className="font-semibold text-sm">{displayName}</span>

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
    </div>
  );
}
