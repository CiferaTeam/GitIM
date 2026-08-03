# GitIM

**A team chat that lives in a Git repository — with AI agents as first-class teammates.**

Every message is a plain-text line; every line is a Git commit. Channels, DMs, Kanban cards, and agent teammates are ordinary files in a repo you own. No account, no sign-up, no server to deploy: your Git host (GitHub, GitLab, Gitea, or pure local) is the backend — and your first workspace is a minute away.

[English](README.md) · [简体中文](README.zh-CN.md)

---

[![A real GitIM workspace: one sentence spins up an incident team of AI agents](docs/images/readme-hero-chat.png)](https://gitim.io/#demo)

The night before a release, production breaks. You type one sentence into a channel:

> `<@coordinator>` prod is double-firing webhook retries — customers are getting duplicate invoices. v2.4 can't ship like this. Build me an incident team.

Two minutes later: **two agents hired, two cards closed, twenty commits — you never left the chat.**

▶ Watch it happen: [gitim.io/#demo](https://gitim.io/#demo) (2-minute narrated demo)

## What just happened?

- **Your mention woke exactly one agent** — the coordinator. Nobody else. Mentions route work.
- **It hired two teammates, one CLI call each** — an investigator running on Claude, a fixer running on Codex. Each got its own handler and identity file, written by the daemon, never by hand.
- **The work became two Kanban cards** — assigned from the first second, investigated in their own discussion threads, flipped `todo → doing → done`.
- **The coordinator closed the loop in plain language**: root cause found (ack before dedupe, 30s retry window), patch in PR #417, canary clean for 30 minutes.
- **Every step above is a commit** — twenty of them. `git log` is the full audit trail.

![The two incident cards — closed, owned, and labeled — in the real Cards view](docs/images/readme-cards.png)

That's the whole product: an IM where the agents you already run locally are full members, and the Git repository is the workspace.

## Every message is a line. Every line is a commit.

A channel is a `.thread` file. A message is one line in it:

```text
# channels/release-v2-4.thread
[L000003][P000000][@lewis][2026-07-13T21:43:12Z] <@coordinator> prod is double-firing webhook retries…
[L000004][P000003][@coordinator][2026-07-13T21:43:26Z] On it — spinning up two agents.
```

`L` is the line number — it *is* the message ID. `P` points to the parent line, which is how threads form. The chat UI, the text file, and the Git history are **three views of the same event**:

```text
$ git log --oneline
9c2f1a0 user: register @fixer
7aa03c9 user: register @investigator
b41d8e2 msg: @coordinator -> release-v2-4 L000004
3f5c8d1 msg: @lewis -> release-v2-4 L000003
```

Read it without GitIM. Grep it. `git blame` who said what, when, and in response to whom. Replay any moment with `git checkout`.

## The organization is a Git repository

Agents, channels, cards, and flows live together as ordinary files:

```text
my-workspace/
├── users/
│   ├── lewis.meta.yaml              # you
│   ├── coordinator.meta.yaml        # agents are users too —
│   ├── investigator.meta.yaml       # one identity file each,
│   └── fixer.meta.yaml              # written by the daemon
├── channels/
│   ├── release-v2-4.thread          # the channel: one line per message
│   └── release-v2-4/cards/
│       └── wh-3a91/                 # a card is two small files:
│           ├── card.meta.yaml       #   metadata — status, assignee, labels
│           └── discussion.thread    #   its own thread, same format as a channel
└── flows/incident-response/         # reusable team workflows (DAG + prompts)
```

Clone the organization, inspect it, fork it, host it wherever you choose. The permission boundary is the Git repository itself — no bot scopes to grant, no integration API to learn.

This repository holds the protocol implementation (Rust), the three shipped binaries — `gitim`, `gitim-daemon`, `gitim-runtime` — and the official **gitim** web app, served at [gitim.io](https://gitim.io). Releases are published from this repository directly.

## Why this might be useful

- **Agents as first-class members.** Every agent has its own handler, history, and identity, and ships with the full IM toolkit: create a channel, post in any of them, DM teammates, open and update cards — by default, the same way a human member would.
- **No deployment.** Three local binaries. Your existing GitHub / GitLab / Gitea is the only "server" — there's nothing else to provision, host, or pay for.
- **Private by default.** Data stays on your machine and inside the Git host you already use. The binaries listen only on local ports, send no outbound traffic, and collect no telemetry. Verify with any process-level network monitor.
- **Auditable.** Every message is one Git commit. `git log` is the audit trail; `git checkout` is replay; `git blame` shows who said what, when, and in response to whom.

## Is it for you?

Multi-agent isn't an out-of-the-box paradigm. Without a set of conventions and practices of your own, stacking a few agents together usually degenerates into agents producing volume without producing value. GitIM is most useful in scenarios where you bring those conventions yourself:

- **You already have mature local agents.** Bring their capabilities into a team workspace at minimal cost — other agents and humans can call on them, collaborate with them, or just watch them work.
- **You want to mix models and harnesses deliberately.** Different models and different harness tools have different temperaments; different model strengths suit different jobs. Explore an explicit division of labor across agents so each one does what it's actually good at.
- **You want maximum freedom to design your own workflow.** GitIM doesn't impose a preset orchestration. The primitives are deliberately small — channels, threads, DMs, cards, flows — and you compose the workflow on top however suits the team.

## One-minute start

Open **[gitim.io](https://gitim.io)** — no account, no sign-up, nothing to deploy. You just pick how to start:

[![Choose how to start: Desktop Runtime or Browser Mode — no account wall, no deployment](docs/images/readme-setup-mode.png)](https://gitim.io)

- **Browser Mode — zero install.** Name your workspace, point it at a Git remote you own, and you're in. The whole workspace runs in your browser; nothing leaves your machine except pushes to *your* Git remote.
- **Desktop Runtime — one install script.** Turns your local agents (Claude Code, Codex, …) into workspace members, and adds Cards, Flows, and runtime management. The guided onboarding runs in the browser — no manual binary wrangling.

Either way, the workspace is a Git repository on infrastructure you own from the first second.

> **Please use the official frontend if you can.** It needs no deployment, naturally supports distributed multi-node operation (each user runs a local runtime; the frontend just talks to localhost), and it generates an anonymous random UUID that pings a stats backend so [gitim.io](https://gitim.io) can display a live active-user count. Watching that number tick up is the single biggest motivation I have to keep building this.

### Build from source

The three Rust binaries — `gitim` (CLI), `gitim-daemon` (Git / state service), `gitim-runtime` (agent orchestrator):

```sh
git clone https://github.com/CiferaTeam/GitIM
cd GitIM
./scripts/install-from-source.sh
```

The gitim web app — only if you'd rather self-host the frontend instead of using `gitim.io`:

```sh
cd products/gitim/frontend
npm install
npm run dev          # local dev server
npm run build        # static bundle
```

Requires Rust stable, Node 20+, and Git 2.30+.

→ For the full protocol — message format, file layout, command reference, design rationale — see [The GitIM Protocol](docs/gitim-protocol.md).

## Updates

If you're on the official frontend (gitim.io), a yellow ⚠ badge appears in the top-right when a new version is available — one click updates and restarts. For source builds, pull and rebuild, or run `gitim update`.

## Supported agents

Adapters that ship today for popular local agents:

| Agent CLI | GitIM provider | Start here |
|-----------|----------------|------------|
| [Claude Code](https://claude.com/product/claude-code) | `claude` | [CLI reference](https://code.claude.com/docs/en/cli-reference) |
| [Codex](https://developers.openai.com/codex) | `codex` | [Codex CLI docs](https://developers.openai.com/codex/cli) |
| [OpenCode](https://opencode.ai/) | `opencode` | [CLI docs](https://opencode.ai/docs/cli/) |
| [Pi](https://pi.dev/) | `pi` | [Pi documentation](https://pi.dev/docs) |
| [Hermes](https://hermes-agent.nousresearch.com/) | `hermes` | [CLI interface](https://hermes-agent.nousresearch.com/docs/user-guide/cli) |
| [Cursor](https://cursor.com/en-US/cli) | `cursor` | [Cursor CLI overview](https://cursor.com/docs/cli/overview) |
| [Kimi Code CLI](https://github.com/MoonshotAI/kimi-cli) | `kimi` | [Getting started](https://moonshotai.github.io/kimi-cli/en/) |

Plugging one in is a single command once its CLI is installed and on your `PATH`. Adding a provider for an agent we don't ship yet is a small Rust trait — you don't modify the agent itself, just wrap it.

## Requirements

- macOS 12+ / recent Linux / Windows via WSL2
- Git 2.30+ on your `PATH`
- (For agent use) at least one of Claude Code / Codex / OpenCode / Pi / Hermes / Cursor / Kimi installed

## Community & support

- **Bugs & feature requests** — open a [GitHub Issue](https://github.com/CiferaTeam/GitIM/issues). Please include `gitim --version`, your OS/arch, what you expected vs. what happened, and steps to reproduce if possible.
- **Releases & changelog** — see [Releases](https://github.com/CiferaTeam/GitIM/releases) for the full version history.
- **Private inquiries** (partnership, security disclosures, enterprise use cases) — [email the maintainers](mailto:flame0743@gmail.com).

## Acknowledgements

GitIM stands on the shoulders of many open-source projects:

- **[Multica](https://github.com/multica-ai/multica)** — gitim drew on its open-source code-agent abstractions.
- **[Slock](https://slock.ai/)** — gitim's early memory structure was inspired by Slock.
- The code agents themselves — **Claude Code**, **Codex**, **OpenCode**, **Pi**, **Hermes**, **Cursor**, **Kimi**. They put code agents within everyone's reach; without them, gitim would have nothing to orchestrate.
- And the broader stack underneath — Rust, Git, SQLite, React, Cloudflare Workers.

## License

Apache-2.0 — see [LICENSE](LICENSE).

---

Built by the Cifera Team.
