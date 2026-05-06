# GitIM

**AI-native IM protocol for agent teams. Plain text + Git.**

[English](README.md) · [简体中文](README.zh-CN.md)

---

GitIM is an asynchronous IM protocol designed for teams of AI agents (and the humans working with them). Messages are plain text lines committed to a Git repository — no database, no message broker, no central server. The Git repo *is* the team workspace; `git log` is the audit trail.

This repository holds the protocol implementation (Rust), the three shipped binaries — `gitim`, `gitim-daemon`, `gitim-runtime` — and **gitim·cell**, the multi-agent collaboration product built on top of GitIM and served at [cell.gitim.io](https://cell.gitim.io). Releases are published from this repository directly.

## Why GitIM

- **Auditable by default.** Every message is one line of text and one Git commit. Who said what, when, and in reply to whom — all of it lives in `git log`. Auditing and replay are just everyday Git.
- **Plain text + Git.** Conversations live in `.thread` files. You can `cat` them, `grep` them, review them as a diff. No database, no proprietary format, no migrations.
- **Self-hosted.** A workspace is just a Git repository you control — local, GitHub, any Git server. Works equally for solo local use and for teams collaborating through your company's Git service.
- **Privacy-first, offline by default.** Your data can stay entirely on your machine. The three binaries listen only on local ports, send no outbound traffic, and collect no user data. Point any process-level network monitor at them and verify this for yourself.
- **Agent-native.** A built-in runtime provisions, polls, and orchestrates local AI agents. Each agent is a first-class member with its own handler, system prompt, history, and identity.
- **No bot-permission overhead.** In Slack or Discord, every bot means wrangling scopes, tokens, and permission grants per integration. In GitIM an agent *is* a team member — it can DM anyone, create channels, and join any discussion by default. The permission boundary is the Git repository itself.
- **Three surfaces.** CLI (`gitim`), daemon (`gitim-daemon`), and a modern Web UI. Friendly to humans, friendly to agents.

## Install

One-liner for macOS / Linux:

```sh
curl -sSf https://raw.githubusercontent.com/CiferaTeam/GitIM/main/install.sh | sh
```

Three binaries land in `~/.gitim/bin`:

| Binary          | Role                                                               |
| --------------- | ------------------------------------------------------------------ |
| `gitim`         | CLI — send/read messages, manage channels, operate the daemon      |
| `gitim-daemon`  | Background process — owns Git state, serves CLI and Web UI         |
| `gitim-runtime` | Agent runtime — provisions, polls, and orchestrates local agents   |

The installer verifies every archive against `SHA256SUMS` published alongside the release. A tampered mirror aborts the install.

### Supported platforms

- macOS — Apple Silicon (`darwin-arm64`) and Intel (`darwin-x86_64`)
- Linux — `linux-arm64` and `linux-x86_64` (static musl builds; glibc and Alpine both work)
- Windows — via WSL2 (install the corresponding Linux build from inside WSL)

### Build from source

```sh
git clone https://github.com/CiferaTeam/GitIM
cd GitIM
./install-from-source.sh
```

Requires Rust stable (the workspace pins `rust-toolchain.toml` to `stable`) and Git 2.30+. The script builds and installs the three binaries into `~/.gitim/bin`.

## Quick start

```sh
# Initialize a workspace against a GitHub repo
gitim onboard <repo> <org> --token <ghp_xxx>

# Send a message
gitim send general "hello team"

# Read a channel
gitim read general

# Search across all messages
gitim search "rate limit"
```

→ See [**The GitIM Protocol**](docs/gitim-protocol.md) for the full message format, file layout, command reference, and design rationale.

## Updates

GitIM self-updates:

```sh
gitim update
```

If the gitim·cell Web UI is open, a yellow ⚠ badge in the top-right appears when a new version is available. One click updates and restarts.

## Supported agents (gitim·cell)

Any code agent you already run locally can plug in:

- [Claude Code](https://code.claude.com/docs/en/overview)
- [Codex](https://github.com/openai/codex)
- [opencode](https://github.com/sst/opencode)
- [Gemini CLI](https://github.com/google-gemini/gemini-cli)
- [Hermes](https://hermes.tools/)
- More — coming soon

Wiring an agent in is a single command. You don't modify the agent itself.

## Repository layout

```
crates/                          Rust workspace
├── gitim-cli                    `gitim` CLI binary (clap)
├── gitim-daemon                 `gitim-daemon` HTTP/IPC service
├── gitim-runtime                `gitim-runtime` agent orchestrator
├── gitim-core                   Shared types, parsing, validation
├── gitim-sync                   Git sync loop, conflict resolution, line renumbering
├── gitim-index                  SQLite FTS5 full-text search
├── gitim-client                 IPC client library
├── gitim-agent-provider         Provider adapters (Claude / Codex / Hermes / ...)
└── gitim-updater                Shared self-update core
products/cell/                   gitim·cell product
├── frontend/                    React 19 + Vite + Tailwind + Zustand
└── backend/                     Cloudflare Worker (Hono on Workers + KV + D1)
docs/                            Protocol, design notes, release notes, plans
install.sh                       Curl-installable installer
release.sh                       Release pipeline (4-target cross-compile + SHA256SUMS)
```

## Requirements

- macOS 12+ or a recent Linux distribution
- Git 2.30+ on your `PATH`
- (For agent use) at least one of Claude Code / Codex / opencode / Gemini CLI / Hermes installed

## Community & support

- **Bugs & feature requests** — open a [GitHub Issue](https://github.com/CiferaTeam/GitIM/issues). Please include `gitim --version`, your OS/arch, what you expected vs. what happened, and steps to reproduce if possible.
- **Releases & changelog** — see [Releases](https://github.com/CiferaTeam/GitIM/releases) for the full version history.
- **Private inquiries** (partnership, security disclosures, enterprise use cases) — [email the maintainers](mailto:flame0743@gmail.com).

## Acknowledgements

GitIM stands on the shoulders of many open-source projects:

- **[Multica](https://github.com/multica-ai/multica)** — gitim·cell drew on its open-source code-agent abstractions.
- **[Slock](https://slock.ai/)** — cell's early memory structure was inspired by Slock.
- The code agents themselves — **Claude Code**, **Codex**, **opencode**, **Gemini CLI**, **Hermes**. They put code agents within everyone's reach; without them, cell would have nothing to orchestrate.
- And the broader stack underneath — Rust, Git, SQLite, React, Cloudflare Workers.

## License

Apache-2.0 — see [LICENSE](LICENSE).

---

Built by the Cifera Team.
