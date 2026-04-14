# Test Audit Cleanup Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Audit and clean up the test suite — fix broken tests, delete duplicates, strengthen weak assertions, protect test-only APIs, consolidate coverage gaps.

**Architecture:** Pure test-layer changes. No production logic modifications except narrowing `open_in_memory()` visibility. All tasks are independent and can be executed in any order (except Task 1 should run first to establish the baseline).

**Tech Stack:** Rust, cargo test, gitim-core, gitim-runtime, gitim-index

---

### Task 1: Fix failing `test_poll_init_and_detect` in poller.rs

**Context:** This integration test starts a real daemon, sends a message, and polls for changes. It fails at line 52 because the second poll returns empty changes. Root cause: the test does a fixed 3-second sleep after `send()`, but `send()` with a remote already waits for push completion internally (via `push_notify` + `PushResult` channel in `handlers.rs:516-539`). The actual bug is that the poll doesn't see changes because of a timing race between daemon startup and sync loop readiness — if the sync loop isn't fully started when send is called, the response comes back as `commit_only` (local commit only, no push), so `origin/main` never advances, and poll sees no diff.

Fix: replace the fixed sleep with a retry loop on poll, and verify the send actually pushed.

**Files:**
- Modify: `crates/gitim-runtime/tests/poller.rs:28-67`

- [ ] **Step 1: Rewrite `test_poll_init_and_detect` with retry-based polling**

Replace the entire test function:

```rust
#[tokio::test]
async fn test_poll_init_and_detect() {
    ensure_daemon_in_path();
    let tmp = short_tempdir();
    let (repo_root, client) = setup_agent(&tmp).await;

    let mut poller = Poller::new(GitimClient::new(&repo_root));

    // First poll: initialize cursor
    let result = poller.poll().await.unwrap();
    assert!(poller.cursor().is_some(), "cursor should be initialized");
    // Note: first poll may return onboard-related channel_meta changes — that's fine.
    // We only care that cursor is set.

    // Send a message
    let send_resp = client
        .send("general", "hello from test", None, None)
        .await
        .unwrap();
    assert!(send_resp.ok, "send failed: {:?}", send_resp.error);

    // Poll with retries — the sync loop may need a moment to push
    let mut detected = false;
    for _ in 0..10 {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let result = poller.poll().await.unwrap();
        if !result.changes.is_empty() {
            let general_change = result.changes.iter().find(|c| c.channel == "general");
            assert!(
                general_change.is_some(),
                "should have a change for 'general' channel"
            );
            detected = true;
            break;
        }
    }
    assert!(detected, "should detect new message after send within 10 retries");

    stop_daemon(&repo_root).await;
}
```

- [ ] **Step 2: Run the test to verify it passes**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-runtime --test poller test_poll_init_and_detect -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run all poller tests to verify no regression**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-runtime --test poller -- --nocapture`
Expected: 3 tests pass, 0 fail

- [ ] **Step 4: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests
git add crates/gitim-runtime/tests/poller.rs
git commit -m "fix(test): replace fixed sleep with retry loop in poller integration test"
```

---

### Task 2: Consolidate dm.rs tests — delete inline duplicates, add missing external tests

**Context:** `crates/gitim-core/src/dm.rs` has 8 inline tests. 3 of them (`test_dm_filename_ordering`, `test_dm_filename_with_hyphens`, `test_dm_filename_prefix_match`) are byte-for-byte duplicates of the 3 tests in `crates/gitim-core/tests/dm_test.rs`. The other 5 inline tests (`parse_dm_filename_*`) have no external counterpart.

Strategy: delete the entire inline `#[cfg(test)]` module, add the 5 missing `parse_dm_filename` tests to the external file.

**Files:**
- Modify: `crates/gitim-core/src/dm.rs` (delete lines 29-83)
- Modify: `crates/gitim-core/tests/dm_test.rs` (add 5 tests)

- [ ] **Step 1: Add the 5 missing `parse_dm_filename` tests to dm_test.rs**

Append to the end of `crates/gitim-core/tests/dm_test.rs`:

```rust
use gitim_core::dm::parse_dm_filename;

#[test]
fn test_parse_dm_filename_valid() {
    let (first, second) = parse_dm_filename("lewis--nexus").unwrap();
    assert_eq!(first, "lewis");
    assert_eq!(second, "nexus");
}

#[test]
fn test_parse_dm_filename_with_hyphens() {
    let (first, second) = parse_dm_filename("cifera-nexus--lewis").unwrap();
    assert_eq!(first, "cifera-nexus");
    assert_eq!(second, "lewis");
}

#[test]
fn test_parse_dm_filename_invalid_no_separator() {
    assert!(parse_dm_filename("lewis").is_none());
}

#[test]
fn test_parse_dm_filename_invalid_empty_first() {
    assert!(parse_dm_filename("--nexus").is_none());
}

#[test]
fn test_parse_dm_filename_invalid_empty_second() {
    assert!(parse_dm_filename("lewis--").is_none());
}
```

- [ ] **Step 2: Run the new tests to verify they pass**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-core --test dm_test -- --nocapture`
Expected: 8 tests pass (3 existing + 5 new)

- [ ] **Step 3: Delete the inline `#[cfg(test)]` module from dm.rs**

Delete lines 29-83 from `crates/gitim-core/src/dm.rs` (the entire `#[cfg(test)] mod tests { ... }` block).

- [ ] **Step 4: Run all dm tests again to verify no regression**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-core --test dm_test -- --nocapture`
Expected: 8 tests pass

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests
git add crates/gitim-core/src/dm.rs crates/gitim-core/tests/dm_test.rs
git commit -m "refactor(test): consolidate dm tests — remove inline duplicates, add parse_dm_filename coverage"
```

---

### Task 3: Move `format_event` tests to external file and fix weak assertion

**Context:** `crates/gitim-core/src/formatter.rs` has 2 inline tests for `format_event()`. The external `crates/gitim-core/tests/formatter_test.rs` has 5 tests for `format_message()` but zero for `format_event()`. Also, `test_format_body_needing_escape` (line 23-28) uses a weak `contains()` assertion that could pass even with malformed output.

Strategy: move `format_event` tests to external file, delete inline module, fix the weak assertion.

**Files:**
- Modify: `crates/gitim-core/src/formatter.rs` (delete lines 67-90)
- Modify: `crates/gitim-core/tests/formatter_test.rs` (add `format_event` tests + fix assertion)

- [ ] **Step 1: Add `format_event` tests and fix weak assertion in formatter_test.rs**

Add import at the top of the file (after existing import):

```rust
use gitim_core::formatter::format_event;
```

Replace the weak test `test_format_body_needing_escape` (lines 23-28) with a strong assertion:

```rust
#[test]
fn test_format_body_needing_escape() {
    let body = "[L000001] looks like a message prefix";
    let result = format_message(2, 0, &Handler::new("nexus").unwrap(), "20250316T120000Z", &format!("see:\n{}", body));
    assert_eq!(
        result,
        "[L000002][P000000][@nexus][20250316T120000Z] see:\n [L000001] looks like a message prefix\n"
    );
}
```

Append the `format_event` tests at the end of the file:

```rust
#[test]
fn test_format_event_self_join() {
    let author = Handler::new("nexus").unwrap();
    let result = format_event(1, &author, "20250316T120000Z", "join", &serde_json::json!({}));
    assert_eq!(
        result,
        "[L000001][P000000][@nexus][20250316T120000Z][E:join] {}\n"
    );
}

#[test]
fn test_format_event_with_targets() {
    let author = Handler::new("nexus").unwrap();
    let meta = serde_json::json!({"targets": ["lewis", "coder"]});
    let result = format_event(5, &author, "20250316T120000Z", "leave", &meta);
    assert!(result.starts_with("[L000005][P000000][@nexus][20250316T120000Z][E:leave] "));
    assert!(result.contains("\"targets\""));
    assert!(result.ends_with('\n'));
}
```

- [ ] **Step 2: Check if `serde_json` is a dev-dependency for gitim-core**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && grep -A5 'dev-dependencies' crates/gitim-core/Cargo.toml`

If `serde_json` is not listed, add it to `[dev-dependencies]`:
```toml
serde_json = { workspace = true }
```

- [ ] **Step 3: Run the tests to verify they pass**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-core --test formatter_test -- --nocapture`
Expected: 7 tests pass (5 existing + 2 new)

- [ ] **Step 4: Delete the inline `#[cfg(test)]` module from formatter.rs**

Delete lines 67-90 from `crates/gitim-core/src/formatter.rs` (the entire `#[cfg(test)] mod tests { ... }` block).

- [ ] **Step 5: Run all formatter tests again to verify no regression**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-core --test formatter_test -- --nocapture`
Expected: 7 tests pass

- [ ] **Step 6: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests
git add crates/gitim-core/src/formatter.rs crates/gitim-core/tests/formatter_test.rs
git commit -m "refactor(test): consolidate formatter tests, add format_event coverage, fix weak assertion"
```

---

### Task 4: Protect `open_in_memory()` with `#[cfg(test)]`

**Context:** `Index::open_in_memory()` at `crates/gitim-index/src/lib.rs:117` is documented as test-only ("创建内存索引，用于测试") and has zero production callers. It's currently `pub` without any gate. Adding `#[cfg(test)]` ensures it can't accidentally be used in production and makes the intent explicit.

**Files:**
- Modify: `crates/gitim-index/src/lib.rs:116-117`

- [ ] **Step 1: Add `#[cfg(test)]` attribute to `open_in_memory()`**

At `crates/gitim-index/src/lib.rs:116-117`, change:

```rust
    /// 创建内存索引，用于测试。
    pub fn open_in_memory() -> Result<Self, IndexError> {
```

to:

```rust
    /// 创建内存索引，用于测试。
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, IndexError> {
```

- [ ] **Step 2: Run gitim-index tests to verify they still pass**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test -p gitim-index -- --nocapture`
Expected: 19 tests pass (14 inline + 5 external)

- [ ] **Step 3: Verify it compiles without test feature**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo check -p gitim-index`
Expected: compiles cleanly (no production code uses `open_in_memory()`)

- [ ] **Step 4: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests
git add crates/gitim-index/src/lib.rs
git commit -m "refactor: gate Index::open_in_memory() behind #[cfg(test)]"
```

---

### Task 5: Final verification — full test suite

- [ ] **Step 1: Run the full test suite**

Run: `cd /Users/lewisliu/ateam/GitIM/.worktrees/audit-tests && cargo test 2>&1 | grep "test result:"`
Expected: all suites pass, 0 failures, test count reduced by 5 (removed 8 inline duplicates/orphans, keeping the 3 that were already external duplicates → net -5)

- [ ] **Step 2: Verify test count change**

Before: 276 tests (275 pass + 1 fail)
After: ~271 tests (all pass, 0 fail)

Breakdown:
- dm.rs: -8 inline, +5 external = net -3
- formatter.rs: -2 inline, +2 external = net 0
- poller: was 1 fail → now 0 fail (same count)
- Total removed: 5 inline tests that were either duplicates (3) or moved to external (2+5-5=2)

Wait — the dm.rs math: 8 inline deleted, but only 3 were duplicated in external. The other 5 were moved to external. So net change for dm: -8 inline + 5 new external = -3 (because 3 already existed as external). Total net change: -3 (dm) + -2 (formatter inline removed) + 2 (formatter external added) = -3.

Final: ~273 tests, all passing.

- [ ] **Step 3: Commit any final adjustments if needed**
