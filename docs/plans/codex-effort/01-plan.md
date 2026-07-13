# Codex Effort Configuration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let Codex agents select a reasoning effort from both the WebUI and `gitim-runtime` CLI, persist it per agent, validate it during provisioning, and pass it to Codex as `model_reasoning_effort`.

**Architecture:** Keep the existing provider-neutral `effort` field as the persisted and HTTP/CLI contract. Enrich Codex's runtime model catalog with each model's advertised default and supported efforts from `codex debug models`. The WebUI resolves effort choices from the selected Codex model, while the provider and provisioning preflight translate a configured value into Codex's `-c model_reasoning_effort="..."` override. An unset effort omits the override and therefore uses the selected model's Codex default.

**Tech Stack:** Rust stable, Tokio, Serde, Clap, React 19, TypeScript, Vitest. No new dependencies.

---

## File Map

| File | Action | Responsibility |
|---|---|---|
| `crates/gitim-agent-provider/src/codex/mod.rs` | Modify | Forward configured effort; omit the override when unset |
| `crates/gitim-agent-provider/src/types.rs` | Modify | Document `ExecOptions.effort` as shared by Claude and Codex |
| `crates/gitim-agent-provider/tests/codex_integration_test.rs` | Modify | Lock configured and default effort argv behavior |
| `crates/gitim-agent-provider/tests/fixtures/mock_codex.sh` | Modify | Assert generic Codex effort expectations |
| `crates/gitim-runtime/src/model_catalog.rs` | Modify | Expose Codex default/supported efforts per model |
| `crates/gitim-runtime/src/preflight.rs` | Modify | Carry Codex effort through add-agent preflight |
| `crates/gitim-runtime/src/http.rs` | Modify | Treat effort as a Claude/Codex field and preflight the selected value |
| `crates/gitim-runtime/src/bin/runtime.rs` | Modify | Document Codex support in `--effort` help |
| `crates/gitim-runtime/src/cli/cmd_add_agent.rs` | Modify | Document the existing add-agent effort wire field |
| `crates/gitim-runtime/src/cli/cmd_update_agent.rs` | Modify | Document the existing update-agent effort wire field |
| `crates/gitim-runtime/tests/model_catalog.rs` | Modify | Verify Codex effort metadata parsing |
| `crates/gitim-runtime/tests/preflight_codex.rs` | Modify | Verify Codex preflight argv includes configured effort |
| `crates/gitim-runtime/tests/preflight_for_add_request.rs` | Modify | Verify dispatcher carries effort into Codex preflight |
| `products/gitim/frontend/src/lib/providers.ts` | Modify | Model effort metadata and resolve model-aware choices |
| `products/gitim/frontend/src/lib/providers.test.ts` | Modify | Verify static/dynamic Codex effort resolution |
| `products/gitim/frontend/src/components/management/add-agent-dialog.tsx` | Modify | Show and submit Codex effort during creation |
| `products/gitim/frontend/src/components/management/agent-detail.tsx` | Modify | Show and edit Codex effort while stopped |
| `products/gitim/frontend/src/components/management/agent-field-lock.ts` | Modify | Lock Codex effort while running |
| `products/gitim/frontend/src/components/management/agent-field-lock.test.ts` | Modify | Verify Codex model/effort lock copy |
| `products/gitim/frontend/src/lib/types.ts` | Modify | Document the shared Claude/Codex effort field |

---

## Task 1: Codex provider execution

- [x] Replace the hard-coded Codex effort test with tests that first fail for an explicit `ultra` override and an unset/default invocation.
- [x] Forward `ExecOptions.effort` as `-c model_reasoning_effort="<value>"` only when present.
- [x] Run `cargo test -p gitim-agent-provider --test codex_integration_test`.

## Task 2: Runtime catalog and provisioning preflight

- [x] Add failing catalog assertions for `default_reasoning_level` and ordered `supported_reasoning_levels` parsing.
- [x] Add failing Codex preflight and dispatcher assertions for the selected effort.
- [x] Extend the model-catalog wire type with optional effort metadata, keeping other provider responses backward-compatible.
- [x] Thread effort through `PreflightOverrides` and add-agent dispatch, then apply Codex's config override during its preflight.
- [x] Update runtime/CLI comments and help text from Claude-only to Claude/Codex.
- [x] Run the scoped runtime catalog, preflight, CLI, and agent-patch tests.

## Task 3: WebUI model-aware effort controls

- [x] Add failing provider helper tests for Sol/Terra/Luna and runtime-catalog overrides.
- [x] Extend `ProviderModel` with optional default/supported effort metadata.
- [x] Resolve Codex options from the selected model, with current Codex effort levels as the custom/default-model fallback.
- [x] Render the effort selector for Claude and Codex in create/edit flows, submit Codex values, and clear an incompatible effort when the model changes.
- [x] Lock Codex effort while a running agent is edited.
- [x] Run targeted frontend tests, then frontend lint and build.

## Task 4: Delivery verification

- [x] Run `cargo fmt --check`.
- [x] Run the scoped Rust tests touched above.
- [x] Because the model-catalog HTTP wire type and preflight dispatch signature are shared runtime contracts, run one final workspace `cargo test`.
- [x] Run `npm test`, `npm run lint`, and `npm run build` in `products/gitim/frontend`.
- [x] Review `git diff --check` and the final diff for current-state-only documentation and comments.

Verification note: `timeline_cap_truncates_runaway_cron` now verifies only the per-cron cap; future/missed classification stays in its dedicated tests. The cap test passed 55 consecutive runs across a live minute boundary, and the complete 14-test cron timeline target passed.
