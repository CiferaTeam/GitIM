# Message Body Renderer Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace plain-text message body rendering with a rich-text renderer that parses GitIM protocol inline formats + markdown formatting into styled React elements.

**Architecture:** A single `parseMessageBody(body)` function splits the raw text into typed fragments, and a `<MessageBody>` component maps fragments to styled React elements. Zero external dependencies.

**Tech Stack:** TypeScript, React

---

## Supported Formats (priority order)

### GitIM Protocol Formats (from specs)
1. **Mention**: `<@handler>` — accent-colored, font-medium
2. **Channel link**: `<#channel>` — accent-colored, clickable
3. **Message link**: `<#channel:LNNNNNN>` — accent-colored, clickable
4. **User profile link**: `<~handler>` — muted accent, clickable
5. **External link**: `<!url>` or `<!url|display text>` — underlined, opens in new tab

### Markdown Formats
6. **Code block**: ` ```lang\n...\n``` ` — mono font, surface background, full-width
7. **Inline code**: `` `code` `` — mono font, surface background, rounded
8. **Bold**: `**text**` — font-bold
9. **Italic**: `*text*` — italic

## File Structure

```
webui-v2/src/
  lib/
    message-parser.ts       # parseMessageBody() — pure function, text → Fragment[]
  components/
    chat/
      message-body.tsx      # <MessageBody> — renders Fragment[] as React elements
      message-item.tsx      # (modify) — replace {message.body} with <MessageBody>
  lib/mock/
    data.ts                 # (modify) — add messages with rich formatting for demo
```

---

## Task 1: Message Parser

**Files:**
- Create: `webui-v2/src/lib/message-parser.ts`

- [ ] **Step 1:** Define the Fragment type union:
  - `TextFragment`: plain text
  - `MentionFragment`: handler string (from `<@handler>`)
  - `ChannelLinkFragment`: channel name (from `<#channel>`)
  - `MessageLinkFragment`: channel + line number (from `<#channel:LNNNNNN>`)
  - `UserProfileFragment`: handler (from `<~handler>`)
  - `ExternalLinkFragment`: url + optional title (from `<!url>` or `<!url|title>`)
  - `CodeBlockFragment`: language (optional) + code content
  - `InlineCodeFragment`: code content
  - `BoldFragment`: inner text (may contain nested fragments)
  - `ItalicFragment`: inner text (may contain nested fragments)

- [ ] **Step 2:** Implement `parseMessageBody(body: string): Fragment[]`

  Parsing strategy — two passes:
  1. **Block pass**: split on code blocks (` ```...``` `) first, since code blocks should not be parsed for other formats
  2. **Inline pass**: for each non-code-block segment, scan left-to-right for:
     - GitIM links: regex `<([@#~!])([^>\n]+)>` — match the prefix to determine type
       - `@` → MentionFragment (validate handler format: `^[a-z0-9]([a-z0-9-]*[a-z0-9])?$`)
       - `#` → check for `:L\d{6,}$` suffix → MessageLinkFragment or ChannelLinkFragment
       - `~` → UserProfileFragment (validate handler format)
       - `!` → ExternalLinkFragment (split on first `|` for title)
     - Inline code: `` `...` `` (backtick pairs, no nesting)
     - Bold: `**...**` (double asterisk pairs)
     - Italic: `*...*` (single asterisk, only when not part of `**`)
     - Plain text: everything between matched patterns

  For bold/italic, the inner content is plain text only (no recursive parsing needed for v1).

- [ ] **Step 3:** Verify build passes

- [ ] **Step 4:** Commit: "feat(webui-v2): message body parser with GitIM protocol + markdown formats"

**Acceptance:** `parseMessageBody("Hello <@alice>, check <#dev-tasks> and <!https://example.com|this link>")` returns an array of correctly typed fragments. Code blocks are not parsed for inner patterns. Invalid mentions/links (bad format) are left as plain text.

---

## Task 2: MessageBody Component

**Files:**
- Create: `webui-v2/src/components/chat/message-body.tsx`

- [ ] **Step 1:** Create `<MessageBody body={string} />` component:
  - Calls `parseMessageBody(body)` (memoized via useMemo)
  - Maps each fragment to a styled React element:

  | Fragment | Rendering |
  |----------|-----------|
  | Text | `<span>{text}</span>` |
  | Mention | `<span class="text-primary font-medium">@{handler}</span>` |
  | ChannelLink | `<span class="text-primary cursor-pointer hover:underline"># {channel}</span>` |
  | MessageLink | `<span class="text-primary cursor-pointer hover:underline"># {channel}:L{line}</span>` |
  | UserProfile | `<span class="text-primary/80 cursor-pointer hover:underline">~{handler}</span>` |
  | ExternalLink | `<a href={url} target="_blank" rel="noopener noreferrer" class="text-primary underline hover:text-primary/80">{title or url}</a>` |
  | InlineCode | `<code class="bg-muted px-1.5 py-0.5 rounded text-[13px] font-mono">{code}</code>` |
  | CodeBlock | `<pre class="bg-muted rounded-md p-3 my-1 overflow-x-auto font-mono text-[13px]"><code>{code}</code></pre>` |
  | Bold | `<strong>{text}</strong>` |
  | Italic | `<em>{text}</em>` |

  - Code blocks render as block-level elements; everything else is inline
  - External links: sanitize URL (only allow http/https protocols)

- [ ] **Step 2:** Verify build passes

- [ ] **Step 3:** Commit: "feat(webui-v2): MessageBody component with styled fragment rendering"

**Acceptance:** `<MessageBody body="Hello **world**" />` renders "Hello " + bold "world". All fragment types render with correct styling per DESIGN.md tokens.

---

## Task 3: Integration & Mock Data

**Files:**
- Modify: `webui-v2/src/components/chat/message-item.tsx`
- Modify: `webui-v2/src/components/chat/thread-panel.tsx`
- Modify: `webui-v2/src/lib/mock/data.ts`

- [ ] **Step 1:** In `message-item.tsx`, replace the plain text body:
  ```
  Before: <p ...>{message.body}</p>
  After:  <div ...><MessageBody body={message.body} /></div>
  ```
  Change `<p>` to `<div>` since MessageBody may contain block-level elements (code blocks).
  Keep all existing click/double-click handlers on the wrapper div.

- [ ] **Step 2:** In `thread-panel.tsx`, find where message body is rendered and replace with `<MessageBody>` as well.

- [ ] **Step 3:** In `mock/data.ts`, update some existing mock messages to use rich formatting:
  - Add a message with mentions: `"Hey <@alice>, can you review this?"`
  - Add a message with channel link: `"Moved this to <#dev-tasks>"`
  - Add a message with external link: `"Check the docs <!https://gitim.dev/docs|GitIM Docs>"`
  - Add a message with inline code: `` "The `sync_loop` function needs a retry backoff" ``
  - Add a message with code block: `` "Found the issue:\n```rust\nfn push() -> Result<()> {\n    // missing retry\n}\n```" ``
  - Add a message with bold/italic: `"This is **critical** and needs *immediate* attention"`
  - Add a message with message link: `"Related discussion at <#general:L000003>"`
  - Add a message with user profile link: `"Assigned to <~bob>"`

- [ ] **Step 4:** Verify build passes and mock messages render with rich formatting

- [ ] **Step 5:** Commit: "feat(webui-v2): integrate MessageBody into chat and add rich mock messages"

**Acceptance:** Mock messages display with colored mentions, clickable links, styled code blocks, bold/italic text. Thread panel also renders rich text. No console errors.
