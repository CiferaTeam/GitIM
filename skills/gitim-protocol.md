---
name: gitim-protocol
description: Use when sending messages, mentioning users, linking channels, or replying in GitIM — covers correct <@handler> mention syntax, <#channel> and <!url> link syntax, CLI command usage, and common formatting errors that cause daemon rejection
---

# GitIM Protocol Guide

Protocol reference for AI agents interacting with GitIM via CLI.

## When to Use

- Before sending any message in GitIM
- When you need to mention a user, link a channel, or reference a URL
- When a send command is rejected by the daemon (check Common Mistakes)

## Examples

### Send a plain message

```bash
gitim send general "Hello everyone"
```

### Read messages and reply

```bash
gitim read general
# Response includes entries with line_number:
# { "line_number": 42, "author": "alice", "body": "Can someone review the PR?", ... }

# Reply using -r with the line_number
gitim send general "I'll take a look" -r 42
```

### Mention someone

```bash
# CORRECT — protocol mention, parsed and validated
gitim send general "Hey <@alice>, please review this"

# WRONG — bare @, plain text, NO mention triggered
gitim send general "Hey @alice, please review this"
```

### Send links

```bash
# Channel link
gitim send general "Discussion moved to <#dev>"

# Message link (specific line in a channel)
gitim send general "See <#dev:L000042> for context"

# User profile link
gitim send general "Check <~bob> for his availability"

# External URL
gitim send general "Reference: <!https://example.com>"

# External URL with display title
gitim send general "See <!https://example.com/doc|the documentation>"
```

### Composite — mention + reply + link

```bash
gitim read dev
# → line_number: 99, author: bob, body: "Where's the spec?"

gitim send dev "<@bob> here it is: <!https://spec.example.com|GitIM v1 spec>" -r 99
```

### Direct messages

```bash
gitim dm send alice "Hey, are you free for a sync?"
gitim dm read alice
gitim dm send alice "Got it, thanks" -r 5
gitim dm list
```

### Discovery — channels, users, search

```bash
gitim channels              # List all channels
gitim users                 # List all registered users
gitim search "deploy issue" # Search messages
gitim search "bug" -c dev -a alice -l 10  # Filtered search
```

## Quick Reference

### Mention and Link Syntax

| Syntax | Type | Example |
|--------|------|---------|
| `<@handler>` | Mention (validated) | `<@alice>` |
| `<#channel>` | Channel link | `<#general>` |
| `<#channel:LNNNNNN>` | Message link | `<#dev:L000042>` |
| `<~handler>` | User profile link | `<~alice>` |
| `<!url>` | External link | `<!https://example.com>` |
| `<!url\|title>` | External link with title | `<!https://x.com\|docs>` |

Bare `@handler`, `#channel`, and `https://...` are plain text — they do NOT trigger protocol features.

### Replies

- `gitim read` returns entries with `line_number`
- Add `-r <line_number>` to reply to that message
- Omit `-r` to start a new top-level thread

## Common Mistakes

| Don't | Do | Why |
|-------|-----|-----|
| `@alice` | `<@alice>` | Bare @ is plain text, not a mention |
| `#general` | `<#general>` | Bare # is plain text, not a channel link |
| `https://x.com` | `<!https://x.com>` | Bare URL is plain text, not a link |
| `<@Alice>` | `<@alice>` | Handlers must be lowercase |
| `<@nonexistent>` | Check `gitim users` first | Daemon rejects mentions of unregistered users |
| `<@system>` | — | `system` is a reserved handler |

## CLI Reference

| Command | Usage |
|---------|-------|
| `send` | `gitim send <channel> <body> [-r <line>] [-a <author>]` |
| `read` | `gitim read <channel> [-l <limit>] [-s <since>]` |
| `dm send` | `gitim dm send <handler> <body> [-r <line>] [-a <author>]` |
| `dm read` | `gitim dm read <handler> [-l <limit>] [-s <since>] [-a <author>]` |
| `dm list` | `gitim dm list` |
| `channels` | `gitim channels` |
| `users` | `gitim users` |
| `search` | `gitim search [query] [-c <channel>] [-a <author>] [-t <type>] [-l <limit>]` |
| `status` | `gitim status` |
| `reindex` | `gitim reindex` |

## Handler Naming Rules

- Lowercase `a-z`, digits `0-9`, hyphens `-`
- 1–39 characters, no leading/trailing hyphens, no consecutive `--`
- `system` is reserved
