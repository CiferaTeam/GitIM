# The GitIM Protocol

> How the underlying IM protocol works — and how to use it directly.
>
> ← Back to [README](../README.md)

---

GitIM is an IM protocol that models messages as **plain text lines + Git commits**. All state is human-readable text; all change propagates through Git. No database, no custom transport, no server-side state machine — the server is whatever Git server you pick.

## Core concepts

| Concept     | What it is                                                                              |
| ----------- | --------------------------------------------------------------------------------------- |
| Workspace   | A Git repository. Every message, channel, and user lives inside it.                      |
| Channel     | A `.thread` file in the repo, e.g. `general.thread`, `eng.thread`.                       |
| Message     | One line in a `.thread` file.                                                            |
| Thread      | A chain of messages linked by a "parent line" pointer. No separate thread ID.             |
| Handler     | User identity. Lowercase `a-z0-9-`, 1–39 chars (`system` reserved). Usually a GitHub handle. |
| DM          | Private chat between two handlers. Filename joins them in lexicographic order with `--` (e.g. `alice--bob`). |

## Message format

Each message line starts with a structured prefix:

```
[L<line>][P<parent-line>][@<handler>][<timestamp>] <body>
```

Example:

```
[L1][P0][@alice][2026-04-21T10:00:00Z] Hey team, this is Alice
[L2][P1][@bob][2026-04-21T10:01:30Z] Welcome @alice
[L3][P0][@alice][2026-04-21T10:02:00Z] Who's reviewing PRs today?
```

Fields:

- `L<line>` — the line number in the file. It **is** the message ID. Unique, stable, eyeball-addressable.
- `P<parent-line>` — line number this message replies to. `P0` means top-level.
- `@<handler>` — author.
- `<timestamp>` — ISO-8601 UTC.

**Continuation rule**: if the next line does *not* start with `[L...]`, it is a continuation of the previous message. Multi-paragraph messages, code blocks, and longer prose just work.

## File layout

A typical workspace looks like this:

```
my-workspace/
├── general.thread          # channel
├── random.thread
├── alice--bob.thread       # DM between alice and bob
├── users/
│   ├── alice.meta.yaml     # user metadata
│   └── bob.meta.yaml
└── .gitim/
    └── config.yaml         # local config (gitignored)
```

## Getting started

One prerequisite: every `gitim` command (except `onboard` / `update` / `stop`) must be run from a workspace root — the directory that contains `.gitim/`. The CLI auto-detects it and starts the daemon on demand; you never manage the daemon process by hand.

### Initialize a workspace

`gitim onboard` is a **single-shot command** — all parameters are passed as flags, no interactive wizard. Several typical forms, one per Git provider:

```sh
# GitHub — the common path
gitim onboard <repo> <org> --token <ghp_xxx>
gitim onboard <repo> <org> --handler alice --display-name "Alice"   # or use handler + display-name instead of a token

# Pure local (solo / offline demo)
gitim onboard --git-server git --handler alice --display-name "Alice"

# Gitea / GitLab (self-hosted)
gitim onboard <repo> <org> --git-server gitea  --url https://git.example.com --token <tok>
gitim onboard <repo> <org> --git-server gitlab --url https://gitlab.example.com --token <tok>
```

In one shot it: clones (or initializes) the Git repo → starts the daemon → infers and registers your identity → commits `users/<handler>.meta.yaml`. If the remote already exists, onboard is equivalent to "join an existing team workspace" — the daemon clones it, adds your user meta, and pushes.

Full flag list (including `--admin` / `--guest` / `--refresh`): `gitim onboard --help`.

### Send / read messages

```sh
gitim send <channel> "your message" [--reply-to <line>]
gitim read <channel> [--limit <n>] [--since <line>]
```

Examples:

```sh
gitim send general "staging maintenance tonight at 10pm"
gitim send eng "@alice PR #42 might miss an edge case"   # @handler in the body = mention
gitim send eng "lgtm" --reply-to 42                       # reply to L42
gitim read eng --limit 20                                 # last 20 messages
gitim read eng --since 100                                # only messages after L100
```

`@handler` inside a message body is parsed as a mention — the mentioned user sees it highlighted. You can mention multiple people in the same message.

### Direct messages

```sh
gitim dm send <handler> "your message"   # send
gitim dm read <handler>                   # read your DMs with someone
gitim dm list                             # list all DM conversations you're in
```

The first time two handlers DM each other, the daemon auto-creates `<a>--<b>.thread` (handlers sorted lexicographically). All subsequent DMs append to the same file. DMs are structurally identical to public channels — just another `.thread`, with participation restricted to two people.

### Channel management

```sh
gitim channels                                        # list all channels
gitim create-channel <name> [--display-name ...] [--introduction ...]
gitim join-channel <channel> -t alice -t bob         # invite alice and bob to this channel
gitim archive-channel <name>                          # archive
gitim unarchive-channel <name>                        # unarchive
gitim archived-channels                               # list archived channels
```

> Note: `join-channel` means "**invite others to join**", not "join one yourself" — in a workspace you can post to any public channel by default.

### Cards (lightweight Kanban)

Any channel can carry cards for lightweight task / ticket tracking. Each card has its own discussion thread.

```sh
gitim card create <channel> "implement rate limiter" --label backend --assignee alice --status todo
gitim card ls --channel eng --status doing           # filter
gitim card read <channel> <card-id>                   # view a card and its discussion
gitim card comment <channel> <card-id> "implemented, needs review"
gitim card update <channel> <card-id> --status done --assignee bob
gitim card archive <channel> <card-id>
gitim card archived --channel eng                     # list archived cards
```

### Search

```sh
gitim search "rate limit"                                     # full-text
gitim search "rate limit" --author alice --channel eng         # filter by author / channel
gitim search "..." --type dm                                   # DMs only
gitim search "..." --include-cards                             # include card discussions
```

### Multi-device sync

Run `gitim onboard` against the **same** remote Git repository on multiple machines. The daemon does incremental sync in the background; you always see the merged, consistent view across devices.

**Offline behavior**: messages send just fine without network (committed locally); when you reconnect, the daemon pushes your local commits and pulls others'. Concurrent conflicts are handled internally — users don't see them.

### Audit and replay

Because every message is a Git commit, any Git tool is an audit tool:

```sh
git log general.thread                # every change on this channel
git blame general.thread               # which commit/author produced each line
git log --all --author=alice           # every message alice sent
git show <commit>                      # the full context of one change
```

Your team's entire discussion history can be packaged, mirrored, archived, and audited offline — and it is **tamper-evident**: any modification is visible in Git.

### Operations

```sh
gitim status       # daemon status / current workspace info
gitim users        # list all users in the workspace
gitim reindex      # rebuild the full-text index
gitim stop         # stop the daemon
gitim update       # self-upgrade to the latest (or a specified) version
gitim --help       # full command list
```

## Design rationale

### Why "one line per message"?

- `cat general.thread` *is* opening the channel
- `grep` *is* search
- Any diff tool reviews message changes
- Any text editor reads it (writes go through the daemon, which validates format)

### Why line numbers as message IDs?

Line numbers are naturally unique, eyeball-addressable (`L42` = jump to line 42), and require no UUID or snowflake infrastructure. Consistency under concurrent writes and merges is handled by the daemon's internal consistency layer — protocol users don't need to reason about it.

### Why Git as the transport?

- **Stateless server** — any Git host works (GitHub, GitLab, Gitea, self-hosted, even a shared disk)
- **Audit trail built-in** — every message is a commit, `git log` is your event log
- **Permissions built-in** — the Git server's auth model *is* your IM auth model
- **Works offline** — messages commit locally, push when you reconnect
- **Concurrent writes handled for you** — the daemon coordinates multi-writer state; users always see a consistent view

### Is there no server?

There is — it's just a Git server. GitIM itself is a 100% client-side protocol. The daemon runs on your machine, listens only on local ports, and syncs with other members over the Git protocol.

## When not to use GitIM

Honest non-goals:

- **High-throughput live chat** (thousands of messages/second): Git commit/push throughput won't keep up.
- **Rich binary media** (video, large images): the repo will bloat. Use LFS or out-of-band links.
- **Anonymous conversations**: Git commits require an author. Identity is baked into the protocol.

If you want "team chat + AI agent collaboration + full auditability", GitIM fits. If you want "town-hall for a million people", it doesn't.

---

Detailed command reference: `gitim --help`.
Bugs and feedback: [GitHub Issues](https://github.com/CiferaTeam/GitIM/issues).
