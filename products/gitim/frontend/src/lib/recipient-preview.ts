import { expandAllMentions } from "./expand-all-mentions";
import type { Channel, Message } from "./types";

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
}: {
  body: string;
  channel: Channel | null | undefined;
  replyTo: Message | null;
  messages: Message[];
}): string[] {
  if (!channel || !body.trim()) return [];

  const recipients = new Set<string>();

  if (channel.kind === "dm") {
    for (const member of channel.members) addRecipient(recipients, member);
    return [...recipients].sort();
  }

  addRecipient(recipients, channel.created_by);
  addParentChainRecipients(recipients, replyTo, messages);
  addMentionRecipients(recipients, expandAllMentions(body, channel.members));

  return [...recipients].sort();
}
