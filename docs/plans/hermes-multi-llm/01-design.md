# Hermes Multi-LLM Provider Selection — Design

**Status:** Spec, awaiting plan
**Owner:** lewis
**Date:** 2026-05-10
**Depends on:** `docs/plans/multi-provider/01-plan.md` (GitIM-layer provider selection),
`docs/plans/hermes-profile-isolation/plan.md` (per-agent hermes profile isolation)

---

## Goal

Let the WebUI pick a specific LLM provider × model when adding a hermes-typed
agent. The newly created hermes profile (`~/.hermes/profiles/gitim-<handler>/`)
gets its `config.yaml.model` subtree written to match the user's choice, so
each agent can run on a different LLM without manual `hermes config set`.

## Background — what already exists

Two adjacent plans set the stage:

- **`multi-provider`** — added GitIM-layer provider selection (`claude` /
  `codex` / `hermes`) to `POST /agents/add`, the `Detect` button on the
  AddAgentDialog, and `GET /preflight/{provider}`. The model dropdown in that
  plan is hard-coded per GitIM provider (`claude-sonnet-4-6` etc.).

- **`hermes-profile-isolation`** — every gitim agent gets a 1:1 hermes profile
  at `~/.hermes/profiles/gitim-<handler>/`, cloned from the user's active
  profile via `hermes profile create --clone --no-alias`. The Non-goals
  explicitly say "WebUI does not surface profile concept" — this plan is the
  first time the WebUI reaches into hermes-internal LLM dimensions.

The `MeJson` struct already carries `provider` / `model` / `system_prompt` /
`env`. The `model` field is dead for hermes today (daemon writes it, hermes
provider doesn't read it into `config.yaml`). This plan introduces parallel
`llm_provider` / `llm_model` fields rather than overload `model`.

## Decisions ledger

| # | Decision | Choice |
|---|----------|--------|
| Q1 | Source of LLM provider list | **Backend introspection** of `~/.hermes/.env` + `~/.hermes/config.yaml.custom_providers` |
| Q2 | OAuth provider support | **Not in v1** — API-key only; `auth.json.providers` not read |
| Q3 | Writing model config to new profile | **Shell out** `hermes -p gitim-<h> config set model.{provider,default,base_url}` |
| Q4 | Per-provider model list source | **Live fetch** `GET <base_url>/models` (OpenAI-compatible); failure → 200 + empty + error → frontend Custom input |
| Q5 | Existing agents + edit capability | **Strict new-only** — same posture as hermes-profile-isolation; no retroactive, no edit, no PATCH |
| L1 | LLM model label in v1 | `label = id` (no display_name beautification) |
| L2 | Model fetch caching | **No cache** — fetch on every provider switch in the dialog |
| L3 | HTTP status of `/models` endpoint | **Always 200** — `error` field carries upstream failure |
| L4 | Endpoint namespace | `/hermes/llm/*` (not under `/agents/...`) |
| L5 | me.json field naming | `llm_provider` / `llm_model` (not overload `model`) |
| L6 | Rollback on config-set failure | `delete_profile` + `cleanup_agent_dir` (no partial state) |
| L7 | Detect upgrade | Pass `llm_provider`/`llm_model` as query params; preflight runs on default profile with `--provider`/`--model` override (no temp profile spawn) |
| L8 | Frontend layout | Inline in existing AddAgentDialog (no separate wizard page) |
| L9 | Dialog state lifetime | Reset all hermes-LLM state on dialog close (mirrors multi-provider Q4d) |

## Architecture

### Endpoints

```
GET  /hermes/llm/providers
     200 { providers: [{ id, label, kind: "api_key"|"custom", base_url? }] }
     # Reads ~/.hermes/.env and ~/.hermes/config.yaml.custom_providers.
     # Order: builtin alphabetic, custom last.

GET  /hermes/llm/providers/{id}/models
     200 { models: [{ id, label }], custom_allowed: true, error: null|string,
           fetched_at_ms: u64 }
     # Live fetch <base_url>/models (5s timeout, no retry, no cache).
     # Failures land in `error`; status stays 200.

GET  /preflight/hermes?llm_provider=<X>&llm_model=<Y>
     # Existing endpoint, two new query params.
     # Runs `hermes chat --provider X --model Y "Reply with: GITIM_OK"` on
     # the default profile (borrows default's credentials for a quick
     # handshake check before the agent profile gets the real config).

POST /agents/add
     body adds: llm_provider?: string, llm_model?: string
     # Required when provider == "hermes"; ignored otherwise.
```

### `MeJson` schema additions

```rust
pub llm_provider: Option<String>,  // populated when provider == "hermes"
pub llm_model: Option<String>,     // same
```

`provider` and `model` fields stay unchanged. Daemon's merge semantics already
handle preserving fields it doesn't know about (`#[serde(flatten)] extra`),
so partial rewrites don't lose these.

### Module layout

```
crates/gitim-runtime/src/hermes_llm/
├── mod.rs
├── registry.rs       # static BUILTIN_PROVIDERS table
├── introspect.rs     # list_providers(hermes_home) -> Vec<LlmProvider>
└── models.rs         # fetch_models(provider, hermes_home) -> ModelListResult
```

`http.rs` adds two route handlers + extends `agents_add` and the existing
`preflight_handler`. `hermes_profile.rs` gains `apply_model_config(handler,
provider, model, base_url)` for the shell-out sequence.

## Introspection logic

`GET /hermes/llm/providers` walks two sources:

**Source 1 — `~/.hermes/.env`:** the static `BUILTIN_PROVIDERS` table holds
each provider's `env_vars` list (one or more aliases — hermes accepts e.g.
`ANTHROPIC_API_KEY` or `ANTHROPIC_TOKEN` or `CLAUDE_CODE_OAUTH_TOKEN`). A
provider is `configured` when **any** of its env_vars in .env has a
non-empty value. Empty values are filtered out.

**Source 2 — `~/.hermes/config.yaml.custom_providers`:** every entry surfaces
as `{ id: "custom:<name>", label: "<name> (custom)", kind: "custom",
base_url: <from entry> }`.

**Failure modes (all return 200 with empty or partial list):**

| Condition | Behavior |
|-----------|----------|
| `~/.hermes/` missing | `{ providers: [] }` |
| `.env` missing or unreadable | Treat as empty, still read `config.yaml` |
| `config.yaml` missing or YAML error | Treat custom_providers as empty; log warn |
| Both .env key and a custom entry of same name exist | Both surface; ids differ (`minimax-cn` vs `custom:minimax-cn`) |

### `BUILTIN_PROVIDERS` (v1, mirrored from `hermes_cli/auth.py:PROVIDER_REGISTRY`)

| id | label | env_vars (any matches) | base_url |
|---|---|---|---|
| `anthropic` | Anthropic / Claude | `ANTHROPIC_API_KEY`, `ANTHROPIC_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN` | `https://api.anthropic.com` |
| `deepseek` | DeepSeek | `DEEPSEEK_API_KEY` | `https://api.deepseek.com/v1` |
| `kimi-coding` | Kimi / Moonshot | `KIMI_API_KEY` | `https://api.moonshot.ai/v1` |
| `minimax` | MiniMax | `MINIMAX_API_KEY` | `https://api.minimax.io/anthropic` |
| `minimax-cn` | MiniMax CN | `MINIMAX_CN_API_KEY` | `https://api.minimaxi.com/anthropic` |
| `zai` | Z.AI / GLM | `GLM_API_KEY`, `ZAI_API_KEY`, `Z_AI_API_KEY` | `https://api.z.ai/api/paas/v4` |

Values mirrored from `hermes_cli/auth.py` at the targeted hermes version.
The Phase 0 baseline task verifies them against the locally installed hermes
binary before implementation starts. Sync cadence: manual PR when hermes
minor version bumps; the `registry.rs` top comment names the version it was
mirrored from. CI does not enforce sync — Custom input is the safety net
for drift.

`openai-codex` and `copilot` are excluded as OAuth (Q2). MiMo is not a
provider — it's a model id (`xiaomi/mimo-v2-pro`) that surfaces under
OpenRouter / ai-gateway / kilocode via custom_providers + live model fetch.

## add_agent flow

### Validation

| GitIM `provider` | `llm_provider` | `llm_model` | Result |
|---|---|---|---|
| `hermes` | missing or empty | any | 400 `"missing llm_provider/llm_model for hermes"` |
| `hermes` | any | missing or empty | 400 same |
| `hermes` | not in BUILTIN_PROVIDERS and not `custom:<name>` | any | 400 `"unknown llm_provider"` |
| `hermes` | `custom:<name>` where `<name>` not in `config.yaml.custom_providers` | any | 400 `"custom provider not found"` |
| `claude` / `codex` | any | any | Fields ignored, existing flow runs |
| `hermes` | valid | any (no model whitelist — Custom input allowed) | New flow runs |

### New flow steps

1. `ensure_profile(handler)` — existing logic, creates `~/.hermes/profiles/gitim-<handler>/`.
2. `hermes -p gitim-<h> config set model.provider <llm_provider>`.
3. `hermes -p gitim-<h> config set model.default <llm_model>`.
4. (Custom provider only) `hermes -p gitim-<h> config set model.base_url <url>` where `<url>` comes from `config.yaml.custom_providers[<name>].base_url`.
5. Write `me.json` with `provider="hermes"`, `llm_provider`, `llm_model`.

**Rollback:** any failure in steps 2–5 triggers `delete_profile(handler)` +
`cleanup_agent_dir(handler)` so a retry with the same handler starts clean.
Without rollback, step 1 would short-circuit to `AlreadyExists` and leave
the broken config in place.

**Phase 0 verification:** `hermes config set` accepting dotted paths
(`model.base_url`) is a load-bearing assumption. The plan's baseline task
must verify this against the local hermes binary before any implementation
work starts. If unsupported, fall back to `serde_yaml` direct edit and
re-evaluate Q3.

## Live fetch — `/models`

### Resolution

```
1. Path param `id`:
   ├─ in BUILTIN_PROVIDERS → static base_url
   ├─ "custom:<name>" → ~/.hermes/config.yaml.custom_providers[<name>].base_url
   └─ otherwise → 400
2. Auth header: read corresponding API key from .env (builtin) or
   custom_providers[<name>].api_key (custom). Send `Authorization: Bearer <key>`.
3. GET <base_url>/models, 5s timeout via reqwest.
4. Parse OpenAI-compatible: response.data[].id → models[].id (label = id).
```

### Failure → 200 with `error` field

| Condition | `error` value |
|---|---|
| API key missing in .env | `"missing api key for <id> — set <ENV_VAR> in ~/.hermes/.env"` |
| Network unreachable | `"network error: <e>"` |
| Timeout > 5s | `"timeout fetching <base_url>/models"` |
| HTTP 401/403 | `"auth failed (HTTP <code>) — verify api key"` |
| HTTP 4xx/5xx other | `"upstream HTTP <code>"` |
| JSON parse / schema mismatch | `"unexpected response schema (not OpenAI-compatible) — use Custom..."` |

The `error` field never contains the API key. Tests assert response strings
do not include the key literal.

## Frontend UX (AddAgentDialog)

When the user picks `Provider = Hermes`, an inline section reveals:

```
┌─ Hermes LLM ───────────────────────────────┐
│ LLM Provider: [— Select —          ▾]      │
│   options: GET /hermes/llm/providers       │
│   empty list → "No LLM providers           │
│   configured. Add an API key to            │
│   ~/.hermes/.env or run hermes setup."     │
│                                             │
│ LLM Model:    [— Select —          ▾]      │
│   options: GET /providers/{id}/models      │
│   error != null → force Custom input mode  │
│   "Custom..." always last in dropdown      │
│                                             │
│ [Detect]  ✓ OK · 850ms                     │
└────────────────────────────────────────────┘
```

### State machine

| GitIM provider | LLM provider | LLM model | Detect | Add |
|---|---|---|---|---|
| empty | n/a | n/a | n/a | disabled |
| `claude` / `codex` | n/a | n/a | (multi-provider plan) | (multi-provider plan) |
| `hermes` | empty | n/a | disabled | disabled |
| `hermes` | set | empty | disabled | disabled |
| `hermes` | set | set | enabled | disabled until Detect succeeds |

### Fetch timing

- `GET /hermes/llm/providers` fires once when GitIM provider switches to `hermes`.
- `GET /providers/{id}/models` fires when LLM provider is selected; switching
  LLM provider clears model selection and re-fetches.
- Dialog close resets all hermes-LLM state.

### Detect upgrade

Detect calls `GET /preflight/hermes?llm_provider=X&llm_model=Y`. Backend runs
`hermes chat --provider X --model Y "Reply with: GITIM_OK"` on the default
profile (no temp profile spawn). This validates the (provider, model) pair
can handshake using default's credentials before the agent profile commits.

## Testing strategy

| Layer | Coverage | Location |
|---|---|---|
| Unit — registry | BUILTIN_PROVIDERS 6 entries: ids unique, env_vars unique, base_urls present, mirror-comment names hermes version | `hermes_llm/registry.rs` inline |
| Unit — introspect | 6 cases: empty .env / present key / empty value / config.yaml missing / YAML parse error / collision (.env + custom same name) | `hermes_llm/introspect.rs` inline, tempdir fixtures |
| Unit — fetch_models | OpenAI-compat success / 401 / 5xx / timeout / JSON parse fail / `data` field missing | `hermes_llm/models.rs` inline, `httpmock` |
| Unit — config-set sequence | All 3 succeed / step 2 fails → delete_profile + cleanup_agent_dir invoked / hermes binary missing → `CliNotFound` | `hermes_profile.rs` with injectable binary path |
| HTTP integration | `/hermes/llm/providers` shape / `/models` 200 + error / `/agents/add` provider=hermes missing llm_* → 400 / happy-path full flow asserts me.json + profile config.yaml | `tests/runtime_http_hermes_llm.rs` (new) |
| Preflight upgrade | `/preflight/hermes?llm_provider=X&llm_model=Y` real run, gated `#[ignore]` | `tests/preflight_hermes.rs` (extend) |
| E2E backend | curl: list providers → list models → add agent → me.json fields written → profile config.yaml model subtree populated | `tests/hermes_llm_e2e.rs` (new) |
| E2E UI | Playwright: pick hermes → LLM dropdown appears → pick minimax-cn → model live fetch → pick model → Detect → Add succeeds → agent in list | `e2e/tests/ui-hermes-llm.spec.ts`, gated `E2E_REAL_PROVIDERS=1` |

`--ignored` and `E2E_REAL_PROVIDERS` gating mirror the multi-provider plan;
unattended CI does not spend on real LLM calls.

## Non-goals

- OAuth-class LLM providers (Nous / openai-codex / copilot)
- Retroactive LLM config for existing agents (manual `hermes -p ... config set` migration documented)
- Editing LLM after agent creation (covered by future PATCH-agent-LLM plan; involves hot-reload + session-migration semantics)
- LLM config syncing via git (per-clone field in me.json, ignored)
- Custom base_url editing UI (user edits `config.yaml.custom_providers` directly)
- Model id beautification or display_name translation
- Static `BUILTIN_PROVIDERS` CI sync against hermes source
- `/models` response caching (TTL or otherwise)
- API key health probing beyond the implicit signal from `/models` 401

## Risks and mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| `hermes config set` rejects dotted paths (`model.base_url`) | Step 4 of new flow fails | Phase 0 baseline task verifies before implementation; fallback is `serde_yaml` direct edit |
| Hermes upgrade renames PROVIDER_REGISTRY env_var | Introspection misses user's provider | Custom input always available; manual PR on hermes minor bump |
| User has stale API key in .env | Introspection lists provider, Detect fails | Accept "listed ≠ usable"; Detect button is the gate |
| API key leaks through error string or log | Security | Tests assert error strings exclude key literal; structured fields, no raw-key logging |
| Step 4 fails leaving partial profile | Next add hits `AlreadyExists` with bad config | Rollback sequence (L6) deletes profile on any failure |
| Hermes binary too old for `config set model.provider` | Phase 0 misses it | Plan's preflight extension reports hermes version; not a hard gate v1 |
| `<base_url>/models` path varies by provider (Anthropic uses `/v1/models`, MiniMax's anthropic-compat endpoint may not expose `/models` at all) | Live fetch returns 404 → falls back to Custom input | Phase 0 baseline probes each builtin's actual `/models` path; if Anthropic needs `/v1/models`, registry stores `models_path: Option<&str>` per provider with default `"/models"`. Failure case still degrades cleanly to error + Custom |

## Out-of-scope dependencies on other work

- This plan assumes `multi-provider` has landed (provider field required on add_agent, Detect button exists).
- This plan assumes `hermes-profile-isolation` has landed (`ensure_profile` / `delete_profile` / `default_profile_ready` exist).
- If either is in flight, sequence them before this one.
