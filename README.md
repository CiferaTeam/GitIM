# GitIM

**Lightweight glue for the AI agents you already use. No deployment, full privacy, auditable.**

[English](README.md) · [简体中文](README.zh-CN.md)

---

GitIM is a small glue layer that lets the AI agents you already run locally collaborate as a team. The Git repository is the workspace, plain text is the wire format, and your existing agents — Claude Code, Codex, opencode, pi, Hermes, whatever you've already invested in — are the participants.

It's deliberately small. Multi-agent isn't a solved paradigm: stack a few chatty agents together and you usually get exactly what it sounds like — agents producing volume without producing value. GitIM doesn't try to fix that. It assumes you already have a mature agent (or a workflow built around one) that earns its keep locally, and gives you the smallest path to bring that local capability into a team setting — no servers to deploy, no proprietary cloud, no rewriting the agent.

This repository holds the protocol implementation (Rust), the three shipped binaries — `gitim`, `gitim-daemon`, `gitim-runtime` — and **gitim·cell**, a collaboration UI built on top of GitIM and served at [cell.gitim.io](https://cell.gitim.io). Releases are published from this repository directly.

## Why this might be useful

Three properties — that's the whole pitch:

- **No deployment.** Three local binaries. Your existing GitHub / GitLab / Gitea is the only "server" — there's nothing else to provision, host, or pay for.
- **Private by default.** Data stays on your machine and inside the Git host you already use. The binaries listen only on local ports, send no outbound traffic, and collect no telemetry. Verify with any process-level network monitor.
- **Auditable.** Every message is one Git commit. `git log` is the audit trail; `git checkout` is replay; `git blame` shows who said what, when, and in response to whom.

If those three are what you need, the rest of this README is install + how to plug your agents in.

## Install

The fastest path is **[gitim.io](https://gitim.io)** — open it in your browser and follow the guided onboarding. It detects your platform, downloads the runtime, and walks you through your first workspace. No terminal, no manual binary management.

> **Please use the official frontend if you can.** It needs no deployment, naturally supports distributed multi-node operation (each user runs a local runtime; the frontend just talks to localhost), and it generates an anonymous random UUID that pings a stats backend so [cell.gitim.io](https://cell.gitim.io) can display a live active-user count. Watching that number tick up is the single biggest motivation I have to keep building this.

### Build from source

The three Rust binaries — `gitim` (CLI), `gitim-daemon` (Git / state service), `gitim-runtime` (agent orchestrator):

```sh
git clone https://github.com/CiferaTeam/GitIM
cd GitIM
./install-from-source.sh
```

The Cell webapp — only if you'd rather self-host the frontend instead of using `cell.gitim.io`:

```sh
cd products/cell/frontend
npm install
npm run dev          # local dev server
npm run build        # static bundle
```

Requires Rust stable, Node 20+, and Git 2.30+.

→ For the full protocol — message format, file layout, command reference, design rationale — see [The GitIM Protocol](docs/gitim-protocol.md).

## Updates

If you're on the official frontend (cell.gitim.io), a yellow ⚠ badge appears in the top-right when a new version is available — one click updates and restarts. For source builds, pull and rebuild, or run `gitim update`.

## Supported agents (gitim·cell)

Adapters that ship today for popular local agents:

- [Claude Code](https://code.claude.com/docs/en/overview)
- [Codex](https://github.com/openai/codex)
- [opencode](https://github.com/sst/opencode)
- [pi](https://github.com/mariozechner/pi-ai)
- [Hermes](https://hermes.tools/)
- More — coming soon

Plugging one in is a single command. Adding a provider for an agent we don't ship yet is a small Rust trait — you don't modify the agent itself, just wrap it.

## Requirements

- macOS 12+ / recent Linux / Windows via WSL2
- Git 2.30+ on your `PATH`
- (For agent use) at least one of Claude Code / Codex / opencode / pi / Hermes installed

## Community & support

- **Bugs & feature requests** — open a [GitHub Issue](https://github.com/CiferaTeam/GitIM/issues). Please include `gitim --version`, your OS/arch, what you expected vs. what happened, and steps to reproduce if possible.
- **Releases & changelog** — see [Releases](https://github.com/CiferaTeam/GitIM/releases) for the full version history.
- **Private inquiries** (partnership, security disclosures, enterprise use cases) — [email the maintainers](mailto:flame0743@gmail.com).

## Acknowledgements

GitIM stands on the shoulders of many open-source projects:

- **[Multica](https://github.com/multica-ai/multica)** — gitim·cell drew on its open-source code-agent abstractions.
- **[Slock](https://slock.ai/)** — cell's early memory structure was inspired by Slock.
- The code agents themselves — **Claude Code**, **Codex**, **opencode**, **pi**, **Hermes**. They put code agents within everyone's reach; without them, cell would have nothing to orchestrate.
- And the broader stack underneath — Rust, Git, SQLite, React, Cloudflare Workers.

## License

Apache-2.0 — see [LICENSE](LICENSE).

---

Built by the Cifera Team.
