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

export function dmChannel(a: string, b: string): string {
  return a <= b ? `dm:${a},${b}` : `dm:${b},${a}`;
}
