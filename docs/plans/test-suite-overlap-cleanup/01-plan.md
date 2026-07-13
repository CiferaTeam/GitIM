# Test Suite Overlap Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce duplicated test ownership and repeated integration setup while preserving each public contract at its narrowest useful layer.

**Architecture:** Protocol validation matrices stay in `gitim-core`; daemon integration tests verify handler-to-error-code mapping and persistence boundaries. Parser tests verify parser wiring, while mention and link edge cases stay with their extractors. E2E files must assert one deterministic product outcome and match the current UI flow.

**Tech Stack:** Rust, Tokio tests, Playwright, Cargo

---

### Task 1: Tighten core test ownership

**Files:**
- Modify: `crates/gitim-core/src/validator/mod.rs`
- Modify: `crates/gitim-core/src/validator/im_rules.rs`
- Modify: `crates/gitim-core/tests/parser_test.rs`
- Modify: `crates/gitim-core/tests/validator_test.rs`

- [x] **Step 1: Make the validator wrapper use the protocol channel type**

Replace the duplicated channel-name rules with delegation to `ChannelName::new`, preserving the existing `ValidationError` reason strings by matching `ChannelNameError`.

- [x] **Step 2: Keep one wrapper-level channel-name contract test**

Replace the valid and invalid input matrices in `validator_test.rs` with one test that checks a successful canonical name and one mapped invalid-name reason. The `ChannelName` unit tests retain the full input matrix.

- [x] **Step 3: Remove extractor edge cases from parser integration tests**

Delete `test_parse_no_mentions`, `test_parse_bare_at_not_extracted`, `test_parse_message_no_links`, and `test_parse_mentions_and_links_independent`. Positive body and continuation tests continue to prove parser wiring; `mention_test.rs` and `link.rs` retain the edge-case contracts.

- [x] **Step 4: Remove the duplicate leave-rule test**

Delete `leave_two_members_self_ok`; it is identical to `leave_self_ok` because both call `validate_leave("alice", &[], USERS, MEMBERS)`.

- [x] **Step 5: Run core tests**

Run: `cargo test -p gitim-core`

Expected: all core unit, integration, and doc tests pass.

### Task 2: Collapse daemon cron validation crossings

**Files:**
- Modify: `crates/gitim-daemon/tests/cron_create_test.rs`

- [x] **Step 1: Replace repeated validation tests with one adapter matrix**

Create a single async test with cases for `invalid_name`, `invalid_schedule`, `invalid_timezone`, `prompt_empty`, and `prompt_too_large`. Reuse one `AppState`; every case must assert failure and the expected daemon `error_code`.

- [x] **Step 2: Reuse one repository for self-target aliases**

Fold the standalone lowercase `@self` test into the alias matrix. Create unique cron names in one repo and assert both the response target and persisted `CronSpec.target` for each alias.

- [x] **Step 3: Run the cron handler target**

Run: `cargo test -p gitim-daemon --test cron_create_test`

Expected: 8 focused tests pass with the same handler contracts.

### Task 3: Remove obsolete and non-deterministic E2E files

**Files:**
- Delete: `e2e/tests/ui-agent-detect.spec.ts`
- Delete: `products/gitim/frontend/e2e/hermes-provider.spec.ts`
- Delete: `products/gitim/frontend/e2e/pi-provider.spec.ts`

- [x] **Step 1: Remove the retired Detect-button scenario**

Delete the real-provider test for the client-side Detect flow. Agent creation now performs server-side preflight in the Add request.

- [x] **Step 2: Remove provider tests that accept both success and failure**

Delete the Hermes and Pi files whose only outcome assertion accepts successful creation or any provider error. Provider preflight shapes remain covered by runtime tests and the real-runtime E2E suite.

- [x] **Step 3: Verify Playwright collection**

Run `npm ci` and `npx playwright test --list` in both `e2e/` and `products/gitim/frontend/`.

Expected: both suites collect successfully and none of the deleted files appear.

### Task 4: Verify the cleanup

**Files:**
- Verify all changed files

- [x] **Step 1: Check formatting and patch hygiene**

Run: `cargo fmt --all -- --check`

Run: `git diff --check`

Expected: both commands exit successfully.

- [x] **Step 2: Re-run scoped behavior tests**

Run: `cargo test -p gitim-core`

Run: `cargo test -p gitim-daemon --test cron_create_test`

Expected: all selected tests pass.

- [x] **Step 3: Review the final diff and test-count delta**

Confirm that production behavior is unchanged, each deleted test has a surviving owner, and the final summary reports Rust test-count and E2E-file deltas.
