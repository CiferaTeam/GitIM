import { expandAllMentions } from "./expand-all-mentions";
import type { Card, Channel, Message } from "./types";

const PROTOCOL_MENTION_RE = /<@([a-z0-9]([a-z0-9-]*[a-z0-9])?)>/g;

function addRecipient(recipients: Set<string>, handler: string | null | undefined) {
  const trimmed = handler?.trim();
  if (trimmed) recipients.add(trimmed);
}

function addParentChainRecipients(
  recipients: Set<string>,
  replyTo: Message | null,
  messages: Message[],
) {
  if (!replyTo || replyTo.line_number <= 0) return;

  const byLine = new Map<number, Message>();
  for (const message of messages) {
    if (message.line_number > 0) byLine.set(message.line_number, message);
  }
  byLine.set(replyTo.line_number, replyTo);

  const visited = new Set<number>();
  let cursor = replyTo.line_number;
  while (cursor > 0 && !visited.has(cursor)) {
    visited.add(cursor);
    const message = byLine.get(cursor);
    if (!message) break;
    addRecipient(recipients, message.author);
    cursor = message.point_to;
  }
}

function addMentionRecipients(recipients: Set<string>, body: string) {
  PROTOCOL_MENTION_RE.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = PROTOCOL_MENTION_RE.exec(body)) !== null) {
    addRecipient(recipients, match[1]);
  }
}

export function computeDraftRecipients({
  body,
  channel,
  replyTo,
  messages,
  excludeSelf,
}: {
  body: string;
  channel: Channel | null | undefined;
  replyTo: Message | null;
  messages: Message[];
  excludeSelf?: string | null;
}): string[] {
  if (!channel || !body.trim()) return [];

  const recipients = new Set<string>();
  const self = excludeSelf?.trim() || undefined;

  if (channel.kind === "dm") {
    for (const member of channel.members) addRecipient(recipients, member);
  } else {
    addRecipient(recipients, channel.created_by);
    addParentChainRecipients(recipients, replyTo, messages);
    addMentionRecipients(
      recipients,
      expandAllMentions(body, channel.members, {
        referenceNonRecipients: true,
        excludeSelf: self,
      }),
    );
  }

  if (self) recipients.delete(self);
  return [...recipients].sort();
}

/**
 * Recipients for a draft message in a card discussion thread.
 *
 * Cards are task records, not chat threads: the daemon's
 * `compute_card_thread_recipients` routes by task role (reporter +
 * assignee) plus explicit `<@handler>` mentions — never the channel
 * membership, and `@all` is not expanded. This mirrors that exactly so
 * the preview matches what the daemon will actually wake.
 */
export function computeCardDraftRecipients({
  body,
  card,
  excludeSelf,
}: {
  body: string;
  card: Pick<Card, "created_by" | "assignee"> | null | undefined;
  excludeSelf?: string | null;
}): string[] {
  if (!card || !body.trim()) return [];

  const recipients = new Set<string>();
  const self = excludeSelf?.trim() || undefined;

  addRecipient(recipients, card.created_by);
  addRecipient(recipients, card.assignee);
  addMentionRecipients(recipients, body);

  if (self) recipients.delete(self);
  return [...recipients].sort();
}
