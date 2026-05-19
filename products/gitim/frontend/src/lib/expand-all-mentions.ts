const ALL_MENTION_RE = /(^|[^\w@-])@all(?=$|[\s,;:!?)]|\]|}|\.(?=$|\s))/gi;
const ALL_PROTOCOL_MENTION_RE = /<@all>/gi;

function concreteMentions(recipients: string[]): string {
  const seen = new Set<string>();
  const mentions: string[] = [];

  for (const recipient of recipients) {
    if (!recipient || seen.has(recipient)) continue;
    seen.add(recipient);
    mentions.push(`<@${recipient}>`);
  }

  return mentions.join(" ");
}

export function expandAllMentions(body: string, recipients: string[]): string {
  const replacement = concreteMentions(recipients);
  if (!replacement) return body;

  return body
    .replace(ALL_PROTOCOL_MENTION_RE, replacement)
    .replace(ALL_MENTION_RE, (_match, prefix: string) => `${prefix}${replacement}`);
}
