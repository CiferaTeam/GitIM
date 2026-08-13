import type { ReactElement } from "react";
import { MessageSquare } from "lucide-react";
import { HoverCard, HoverCardContent, HoverCardTrigger } from "@/components/ui/hover-card";
import { useAgentStore } from "../../hooks/use-agent-store";
import { useChatStore } from "../../hooks/use-chat-store";
import { useFleetStore } from "../../hooks/use-fleet-store";
import { agentModelLabel } from "../management/agent-model-label";
import { PROVIDERS, type ProviderId } from "@/lib/providers";
import type { Agent } from "@/lib/types";
import { normalizeUserKind } from "@/lib/agents-roster";
import { HandlerName } from "./handler-name";

interface UserCardProps {
  handler: string;
  children: ReactElement;
  onStartDm?: (handler: string) => void;
}

function initials(name: string) {
  return name.slice(0, 2).toUpperCase();
}

function avatarColor(name: string) {
  const hues = [210, 150, 30, 280, 340, 190, 45, 260];
  let hash = 0;
  for (let i = 0; i < name.length; i++) hash = name.charCodeAt(i) + ((hash << 5) - hash);
  const hue = hues[Math.abs(hash) % hues.length];
  return `hsl(${hue} 70% 55%)`;
}

function providerLabel(provider: ProviderId | undefined): string | null {
  if (!provider) return null;
  return PROVIDERS[provider]?.label ?? provider;
}

function modelLine(agent: Agent): string {
  const provider = providerLabel(agent.provider);
  const model = agentModelLabel(agent);
  return provider ? `${provider} · ${model}` : model;
}

function resolveAgent(
  handler: string,
  localAgents: Agent[],
  fleetAgents: Array<{ agent: Agent }>,
): Agent | undefined {
  return (
    localAgents.find((a) => a.handler === handler) ??
    fleetAgents.find((s) => s.agent.handler === handler)?.agent
  );
}

export function UserCardContent({
  handler,
  onStartDm,
}: {
  handler: string;
  onStartDm?: (handler: string) => void;
}) {
  const currentUser = useChatStore((s) => s.currentUser);
  const userInfo = useChatStore((s) => s.userInfos.find((u) => u.handler === handler));
  const localAgents = useAgentStore((s) => s.agents);
  const fleetAgents = useFleetStore((s) => s.agents);
  const agent = resolveAgent(handler, localAgents, fleetAgents);
  const kind = agent ? "agent" : normalizeUserKind(userInfo?.kind);
  const roleLabel = kind === "agent" ? "Agent" : kind === "human" ? "Human" : "User";
  const intro = agent?.introduction?.trim();
  const canDm = !!onStartDm && handler !== currentUser;

  return (
    <div data-testid="user-card" className="flex flex-col gap-2.5">
      <div className="flex items-center gap-2.5">
        <div
          className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-[10px] font-bold text-white shadow-sm"
          style={{ backgroundColor: avatarColor(handler) }}
        >
          {initials(handler)}
        </div>
        <div className="min-w-0">
          <div className="truncate text-sm font-medium text-foreground">
            <HandlerName handler={handler} />
          </div>
          <div className="text-[11px] text-muted-foreground">{roleLabel}</div>
        </div>
      </div>

      {agent && (
        <div
          data-testid="user-card-model"
          className="break-words font-mono text-[11px] leading-5 text-text-secondary"
          title={modelLine(agent)}
        >
          {modelLine(agent)}
        </div>
      )}

      {intro && (
        <p className="line-clamp-3 text-[11px] leading-5 text-text-muted">{intro}</p>
      )}

      {canDm && (
        <button
          type="button"
          data-testid="user-card-dm"
          onClick={(e) => {
            e.stopPropagation();
            onStartDm?.(handler);
          }}
          className="flex w-full items-center justify-center gap-1.5 rounded-md bg-primary/15 px-2 py-1.5 text-xs font-medium text-primary transition-colors hover:bg-primary/25"
        >
          <MessageSquare className="h-3.5 w-3.5" />
          <span>Send message</span>
        </button>
      )}
    </div>
  );
}

export function UserCard({ handler, children, onStartDm }: UserCardProps) {
  return (
    <HoverCard openDelay={180} closeDelay={140}>
      <HoverCardTrigger asChild>{children}</HoverCardTrigger>
      <HoverCardContent
        align="start"
        side="bottom"
        sideOffset={6}
        className="w-56"
      >
        <UserCardContent handler={handler} onStartDm={onStartDm} />
      </HoverCardContent>
    </HoverCard>
  );
}
