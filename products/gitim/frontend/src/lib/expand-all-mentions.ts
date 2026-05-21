const ALL_MENTION_RE = /(^|[^\w@-])@all(?=$|[\s,;:!?)]|\]|}|\.(?=$|\s))/gi;
const ALL_PROTOCOL_MENTION_RE = /<@all>/gi;
const PROTOCOL_MENTION_RE = /<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>/g;

interface ExpandAllMentionOptions {
  referenceNonRecipients?: boolean;
}

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

export function expandAllMentions(
  body: string,
  recipients: string[],
  options: ExpandAllMentionOptions = {},
): string {
  const replacement = concreteMentions(recipients);
  const recipientSet = new Set(recipients.filter(Boolean));

  const expanded = replacement
    ? body
        .replace(ALL_PROTOCOL_MENTION_RE, replacement)
        .replace(ALL_MENTION_RE, (_match, prefix: string) => `${prefix}${replacement}`)
    : body;

  if (!options.referenceNonRecipients) return expanded;

  return expanded.replace(PROTOCOL_MENTION_RE, (match, handler: string) =>
    recipientSet.has(handler) ? match : `<~${handler}>`
  );
}
