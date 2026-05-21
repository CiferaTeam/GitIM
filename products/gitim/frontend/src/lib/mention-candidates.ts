export function buildMentionCandidates({
  users,
  agents,
  includeAll,
}: {
  users: string[];
  agents: string[];
  includeAll: boolean;
}): string[] {
  const seen = new Set<string>();
  const candidates: string[] = [];

  function add(candidate: string) {
    const trimmed = candidate.trim();
    if (!trimmed || seen.has(trimmed)) return;
    seen.add(trimmed);
    candidates.push(trimmed);
  }

  if (includeAll) add("all");
  for (const user of users) add(user);
  for (const agent of agents) add(agent);

  return candidates;
}
