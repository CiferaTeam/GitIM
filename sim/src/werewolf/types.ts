export enum Role {
  Wolf = "wolf",
  Seer = "seer",
  Witch = "witch",
  Hunter = "hunter",
  Villager = "villager",
  God = "god",
}

export interface AgentConfig {
  handler: string;
  displayName: string;
  role: Role;
  personality: string;
}

export interface GameConfig {
  players: AgentConfig[];
  daemonUrl: string;
  llmModel: string;
}

function dmChannel(a: string, b: string): string {
  return a <= b ? `dm:${a},${b}` : `dm:${b},${a}`;
}

export function getVisibleChannels(
  handler: string,
  role: Role,
  wolfHandlers: string[],
  allHandlers?: string[]
): string[] {
  const channels: string[] = ["general"];
  channels.push(dmChannel(handler, handler));

  if (role === Role.God) {
    channels.push("wolves");
    if (allHandlers) {
      for (const h of allHandlers) {
        channels.push(dmChannel("god", h));
        channels.push(dmChannel(h, h));
      }
    }
    return channels;
  }

  if (role === Role.Wolf) {
    channels.push("wolves");
  }

  if (role !== Role.Villager) {
    channels.push(dmChannel(handler, "god"));
  }

  return channels;
}
