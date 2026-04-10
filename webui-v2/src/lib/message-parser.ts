export type Fragment =
  | { type: "text"; content: string }
  | { type: "mention"; handler: string }
  | { type: "channel-link"; channel: string }
  | { type: "message-link"; channel: string; line: number }
  | { type: "user-profile"; handler: string }
  | { type: "external-link"; url: string; title?: string }
  | { type: "code-block"; language?: string; code: string }
  | { type: "inline-code"; code: string }
  | { type: "bold"; content: string }
  | { type: "italic"; content: string };

// Handler validation: lowercase a-z 0-9 hyphen, 1-39 chars
// Must not start/end with hyphen
const HANDLER_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?$/;
const CHANNEL_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;

function isValidHandler(s: string): boolean {
  return s.length >= 1 && s.length <= 39 && HANDLER_RE.test(s);
}

function isValidChannel(s: string): boolean {
  return CHANNEL_RE.test(s);
}

// Combined inline pattern — ordered by priority:
// 1. GitIM links: <[@#~!]content>
// 2. Inline code: `code`
// 3. Bold: **text**
// 4. Italic: *text* (not part of **)
const INLINE_RE =
  /<([#~!@])([^>\n]+)>|`([^`\n]+)`|\*\*(.+?)\*\*|(?<!\*)\*(?!\*)(.+?)(?<!\*)\*(?!\*)/g;

function parseGitimLink(prefix: string, content: string): Fragment | null {
  if (prefix === "@") {
    if (!isValidHandler(content)) return null;
    return { type: "mention", handler: content };
  }

  if (prefix === "~") {
    if (!isValidHandler(content)) return null;
    return { type: "user-profile", handler: content };
  }

  if (prefix === "!") {
    const pipeIdx = content.indexOf("|");
    const url = pipeIdx === -1 ? content : content.slice(0, pipeIdx);
    const title = pipeIdx === -1 ? undefined : content.slice(pipeIdx + 1);
    if (!url.startsWith("http://") && !url.startsWith("https://")) return null;
    return title
      ? { type: "external-link", url, title }
      : { type: "external-link", url };
  }

  if (prefix === "#") {
    // Check for message-link suffix :LNNNNNN (6+ digits)
    const msgMatch = content.match(/^(.+):L(\d{6,})$/);
    if (msgMatch) {
      const channel = msgMatch[1];
      const line = parseInt(msgMatch[2], 10);
      if (!isValidChannel(channel)) return null;
      return { type: "message-link", channel, line };
    }
    if (!isValidChannel(content)) return null;
    return { type: "channel-link", channel: content };
  }

  return null;
}

function parseInline(text: string): Fragment[] {
  const fragments: Fragment[] = [];
  let lastIndex = 0;
  INLINE_RE.lastIndex = 0;

  let match: RegExpExecArray | null;
  while ((match = INLINE_RE.exec(text)) !== null) {
    // Text before this match
    if (match.index > lastIndex) {
      fragments.push({ type: "text", content: text.slice(lastIndex, match.index) });
    }

    const [full, gitimPrefix, gitimContent, inlineCode, boldContent, italicContent] = match;

    if (gitimPrefix !== undefined) {
      // GitIM link: <prefix content>
      const fragment = parseGitimLink(gitimPrefix, gitimContent);
      if (fragment) {
        fragments.push(fragment);
      } else {
        // Invalid format — emit as plain text
        fragments.push({ type: "text", content: full });
      }
    } else if (inlineCode !== undefined) {
      fragments.push({ type: "inline-code", code: inlineCode });
    } else if (boldContent !== undefined) {
      fragments.push({ type: "bold", content: boldContent });
    } else if (italicContent !== undefined) {
      fragments.push({ type: "italic", content: italicContent });
    }

    lastIndex = INLINE_RE.lastIndex;
  }

  // Remaining text after last match
  if (lastIndex < text.length) {
    fragments.push({ type: "text", content: text.slice(lastIndex) });
  }

  return fragments;
}

// Pass 1: split on code blocks, then pass 2 on non-code segments
const CODE_BLOCK_RE = /```(\w*)\n([\s\S]*?)```/g;

export function parseMessageBody(body: string): Fragment[] {
  if (body === "") {
    return [{ type: "text", content: "" }];
  }

  const fragments: Fragment[] = [];
  let lastIndex = 0;
  CODE_BLOCK_RE.lastIndex = 0;

  let match: RegExpExecArray | null;
  while ((match = CODE_BLOCK_RE.exec(body)) !== null) {
    // Non-code segment before this code block
    if (match.index > lastIndex) {
      const segment = body.slice(lastIndex, match.index);
      fragments.push(...parseInline(segment));
    }

    const language = match[1] || undefined;
    const code = match[2];
    fragments.push({ type: "code-block", language, code });

    lastIndex = CODE_BLOCK_RE.lastIndex;
  }

  // Remaining text after last code block
  if (lastIndex < body.length) {
    fragments.push(...parseInline(body.slice(lastIndex)));
  }

  return fragments;
}
