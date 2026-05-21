# Snapshot Pack — Phase B 实施 Plan(Auto-Rotate)

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Design 源文档**: [`docs/plans/2026-05-06-git-history-snapshot-pack.md`](../2026-05-06-git-history-snapshot-pack.md) — 本 plan 重新解读 spec 的 Phase B + Phase C,合并成 "runtime 自主 auto-rotate" 一个机制。
>
> **前置 PR(已 land)**:
> - PR #7(Phase A types): `EpochFile` / `EpochStatus` / `SnapshotInfo` / `RedirectInfo` / `ArchiveInfo` 类型 + YAML round-trip + schema 校验,落 `gitim-core::epoch`
> - PR #14(Phase A observability): `AppState::epoch_status` + `refresh_epoch_status` + `is_redirected` + boot/sync 刷新 + status API 暴露
>
> **Phase A enforcement 不再做**:用户裁决:redirect 由 runtime 自己产生,daemon 看到自己写的 redirect 主动切换,不需要 25 handler gate + cron 短路 + error envelope。原 spike branch 的 `e95164a`(write-gate) + `5b2e87b`(cron 短路) 不会进新 PR 序列。

**Goal:** 让 GitIM runtime 在当前 epoch branch 的 commit count 跨过阈值时**自动**完成 epoch 切换 —— 在新 branch 上写 orphan snapshot,在老 branch 上写 redirect commit,原子 push,本地 checkout 新 branch,落本地 bundle 归档(不上传远端)。其他 daemon 同步发现 redirect 后自动 follow。整个流程对 user 无感(秒级写阻塞窗口)。

**Architecture:**
- `gitim-core::epoch` 补一组**写**侧 API(active / redirect 构造器 + `save_to_path` 原子写)。
- `gitim-sync` 加一组 git 原语:数 commit、orphan snapshot commit、redirect commit、`git push --atomic` 两 ref、`git bundle create` 落地。
- `gitim-sync::rotate`(新模块) 把 fire 路径 + follow 路径用现成原语拼起来,**Race 用 `git push --atomic` 单 winner 兜底,不引入额外协调**。
- `AppState` 在 sync_loop 的 `on_pushed`(尝试 fire) + `on_synced`(发现 redirect → follow)两处挂钩。
- 整个 mechanism 在 `commit_lock` 内串行,handler 写入路径**无需任何 enforcement gate** —— follow 完成后 checkout 已经在新 branch,handler 自动写到正确的 branch。

**Tech Stack:** Rust, `serde_yaml`,`std::sync::Mutex`(commit_lock),`tokio::task::spawn_blocking`(git 调用),`tempfile`(原子写 + bundle 测试),`cargo test`.

**v1 写死的开关(用户已 confirm)**:

| 开关 | 值 |
|---|---|
| 阈值 | `1_000_000` commits(环境变量 `GITIM_ROTATION_THRESHOLD` 覆盖,仅 test/debug) |
| Bundle 上传 | **不上传** —— 只 `git bundle create` 到 `<repo_root>/.gitim/archive/epoch-N.bundle` |
| 保留 epoch | 全留,不 prune |
| Replay | 不需要 —— 整个 fire/follow 在 `commit_lock` 内,无 in-flight unpushed commits |
| Fire 失败 | 不重试,下次 push 后再尝试 |

**非目标(留给后续 plan)**:
- Bundle 上传到外部 store(GitHub Release / S3 / LFS)
- Auto-prune 老 epoch(删 ref + gc)
- `/runtime/health` 之外的可观测性(metrics dashboard)
- daemon-web TS port 的 auto-rotate 并行实现(browser 端独立路径)
- WebUI 暴露 epoch 状态 / 手动 rotate 触发
- 多 epoch 共存的索引 / 搜索行为(`gitim-index` 当前不跨 epoch 搜)

---

## 关键设计决策

### 1. 触发点:`on_pushed` 后

每次 sync_loop push 成功后,新 epoch 长度可能跨阈值。`on_pushed` callback 是天然的 rate limit 点 —— 不需要单独 background scheduler。

`on_pushed` 在 sync_loop 内同步触发,callback 本身不持 `commit_lock`(注释明确说网络 op 不持 lock)。所以 callback 内可以 acquire lock 做 rotation,不会死锁。

### 2. Single-writer:`git push --atomic` 兜底

多 daemon 同时跨阈值 → 都 try fire → 都构造好本地的 orphan + redirect → 都尝试 `git push --atomic origin <new-branch>:refs/heads/<new-branch> <old-branch>:refs/heads/<old-branch>`。

GitHub server 串行处理 atomic push:
- 第一个赢家:两个 ref 都更新
- 后到者:`<old-branch>` 推 reject(non-fast-forward,因为赢家已经在 `<old-branch>` 上加了 redirect commit),整个 atomic 失败,**两个 ref 都没动**
- 后到者发现 reject → reset local + checkout 赢家的新 branch(follow path)

零额外协调。git 自己就是 distributed consensus 的载体。

### 3. 无 replay

`commit_lock` 在 fire 全程持有。fire 期间所有 handler 写阻塞。fire 结束(win 或 lose)前 checkout 已经切到新 branch。handler 醒来后自动写到新 branch。

**没有 in-flight unpushed commits 需要 replay**。这跟 spec 原 Phase C "本地未推送 replay queue" 不同 —— 通过 `on_pushed` 触发点 + `commit_lock` 全程持有,直接消除了 replay 场景。

### 4. Follow 路径独立于 fire

不同 daemon 可能没自己 fire,但 sync_loop 在 `pull_rebase` 时会从 remote 拉到别人的 redirect commit。`on_synced` 在每次 cycle 末尾 refresh `epoch_status` —— refresh 后如果 status==Redirected,主动调 `follow_redirect`。

Follow 也持 `commit_lock`,保证 handler 不会在 checkout 切换期间写到老 branch。

### 5. Epoch 编号 + 分支命名

| 状态 | 当前 branch | 下一个 branch |
|---|---|---|
| Legacy / epoch 1 | `main` | `main-epoch-2` |
| epoch N (N≥2) | `main-epoch-N` | `main-epoch-{N+1}` |

Legacy `main` 没有 `gitim.epoch.yaml`,视为 epoch=1。Fire 时:
- 老 branch `main` 写 epoch.yaml(status=redirected, target=main-epoch-2)
- 新 branch `main-epoch-2` orphan commit 含 epoch.yaml(status=active, epoch=2)

epoch 编号从老 branch 的 epoch.yaml 读(没有就当 1),新 epoch 编号 = 老 + 1。

### 6. Archive tag

Fire 成功后(win 路径)在老 branch 的 redirect commit 上打 `archive/epoch-N/<short>` tag。N = 被 sealed 的 epoch,`<short>` = 老 branch sealed commit 的 7 位 short hash。这跟 spec `archive.tag` 字段保持一致。

Tag 由 winner 打,push 到 remote。Follow 路径不打 tag(避免重复)。

### 7. Bundle 本地落地

Fire 成功(win)后,winner 调 `git bundle create <workspace>/.gitim-runtime/archive/epoch-{N}.bundle refs/tags/archive/epoch-{N}/<short>`。

Loser / Follower 不 bundle(他们没"赢得"这次 rotation,bundle 由 winner 负责)。如果 winner 的 bundle 没成功落本地,best-effort warn,不阻塞 rotation。

后续若要"补 archive",每个 daemon 可以 idempotent 检查本地缺失的 epoch bundle 并按需补 —— 这是 future work,本 phase 不做。

---

## File Map

| File | Action | 职责 |
|---|---|---|
| `crates/gitim-core/src/epoch.rs` | Modify | 加 `EpochFile::new_active` / `new_redirect` 构造器 + `save_to_path` 原子写 |
| `crates/gitim-core/tests/epoch_parse.rs` | Modify | 加构造器 + save_to_path round-trip 测试 |
| `crates/gitim-sync/src/git.rs` | Modify | 加 `count_commits_on_branch` / `create_orphan_commit` / `write_redirect_commit` / `atomic_push_two_refs` / `bundle_to_path` / `tag_archive` helpers |
| `crates/gitim-sync/tests/git_ops_test.rs` | Modify | 各 helper 的 unit 测试 |
| `crates/gitim-sync/src/rotate.rs` | Create | `RotationOutcome` 枚举 + `try_fire_rotation` + `follow_redirect` |
| `crates/gitim-sync/src/lib.rs` | Modify | `pub mod rotate;` |
| `crates/gitim-sync/tests/rotate_test.rs` | Create | Solo fire / 双 daemon race / pure follow scenarios |
| `crates/gitim-daemon/src/state.rs` | Modify | `ROTATION_THRESHOLD` 常量 + `on_pushed` 挂 try_fire + `on_synced` 挂 follow |
| `crates/gitim-daemon/tests/epoch_rotation.rs` | Create | 端到端 integration test(threshold override) |
| `crates/gitim-runtime/src/http.rs` | Modify | `/runtime/health` 加 `epoch_count` + `total_commit_count` |
| `crates/gitim-runtime/tests/cli_health.rs`(或现有 health test) | Modify | 新字段断言 |
| `CLAUDE.md` | Modify | Current Orientation 加 Phase B 段 |

---

## Tasks

### Task 1: gitim-core 加 EpochFile 构造器 + save_to_path

**Files:**
- Modify: `crates/gitim-core/src/epoch.rs`
- Modify: `crates/gitim-core/tests/epoch_parse.rs`

PR 1 只加了 `load_from_path` —— rotation 需要把 active / redirect 两种 file 写盘。补一组**显式构造器** + 原子写。

- [ ] **Step 1: Write failing tests for constructors + save round-trip**

Append to `crates/gitim-core/tests/epoch_parse.rs`:

```rust
use gitim_core::epoch::{EpochFile, EpochStatus};
use std::path::PathBuf;
use tempfile::TempDir;

#[test]
fn new_active_constructs_valid_active_file() {
    let f = EpochFile::new_active(
        2,
        "main-epoch-2".to_string(),
        "main".to_string(),
        "aabbccddeeff00112233445566778899aabbccdd".to_string(),
        "1122334455667788990011223344556677889900".to_string(),
        "2026-05-21T00:00:00Z".to_string(),
        Some(("archive/epoch-1/aabbccdd".to_string(), "0".repeat(64))),
    );
    assert_eq!(f.status, EpochStatus::Active);
    assert_eq!(f.epoch, 2);
    assert_eq!(f.branch, "main-epoch-2");
    assert!(f.snapshot.is_some());
    assert!(f.redirect.is_none());
    f.validate().expect("constructed active should validate");
}

#[test]
fn new_redirect_constructs_valid_redirected_file() {
    let f = EpochFile::new_redirect(
        1,
        "main".to_string(),
        2,
        "main-epoch-2".to_string(),
        "1122334455667788990011223344556677889900".to_string(),
        "aabbccddeeff00112233445566778899aabbccdd".to_string(),
        "2026-05-21T00:00:00Z".to_string(),
        Some(("archive/epoch-1/aabbccdd".to_string(), "0".repeat(64))),
    );
    assert_eq!(f.status, EpochStatus::Redirected);
    assert_eq!(f.epoch, 1);
    assert_eq!(f.branch, "main");
    assert!(f.redirect.is_some());
    assert!(f.snapshot.is_none());
    f.validate().expect("constructed redirect should validate");
}

#[test]
fn save_to_path_round_trip() {
    let tmp = TempDir::new().unwrap();
    let path: PathBuf = tmp.path().join("gitim.epoch.yaml");

    let f = EpochFile::new_active(
        2,
        "main-epoch-2".to_string(),
        "main".to_string(),
        "a".repeat(40),
        "b".repeat(40),
        "2026-05-21T00:00:00Z".to_string(),
        None,
    );
    f.save_to_path(&path).expect("save");
    assert!(path.exists());

    let loaded = EpochFile::load_from_path(&path).expect("load").expect("present");
    assert_eq!(loaded.status, EpochStatus::Active);
    assert_eq!(loaded.epoch, 2);
    assert_eq!(loaded.branch, "main-epoch-2");
    assert!(loaded.archive.is_none());
}

#[test]
fn save_to_path_is_atomic_no_partial_on_existing() {
    // Existing valid file is preserved on overwrite (atomic rename).
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("gitim.epoch.yaml");
    let f1 = EpochFile::new_active(
        1, "main".to_string(), "main".to_string(),
        "a".repeat(40), "b".repeat(40),
        "2026-05-21T00:00:00Z".to_string(), None,
    );
    f1.save_to_path(&path).unwrap();

    let f2 = EpochFile::new_redirect(
        1, "main".to_string(), 2, "main-epoch-2".to_string(),
        "c".repeat(40), "d".repeat(40),
        "2026-05-21T01:00:00Z".to_string(), None,
    );
    f2.save_to_path(&path).unwrap();

    let loaded = EpochFile::load_from_path(&path).unwrap().unwrap();
    assert_eq!(loaded.status, EpochStatus::Redirected);
    let redirect = loaded.redirect.as_ref().unwrap();
    assert_eq!(redirect.target_branch, "main-epoch-2");
}
```

- [ ] **Step 2: Run tests to verify failures**

```bash
cargo test -p gitim-core --test epoch_parse new_active_constructs 2>&1 | tail -5
cargo test -p gitim-core --test epoch_parse save_to_path 2>&1 | tail -5
```

Expected: 4 compilation errors — `new_active` / `new_redirect` / `save_to_path` not found.

- [ ] **Step 3: Implement constructors + save_to_path**

Append to `crates/gitim-core/src/epoch.rs`:

```rust
impl EpochFile {
    /// Build an Active-state epoch file pointing at `branch` with `source_*`
    /// describing the commit that was sealed as snapshot ancestor.
    #[allow(clippy::too_many_arguments)]
    pub fn new_active(
        epoch: u32,
        branch: String,
        source_branch: String,
        source_commit: String,
        commit: String,
        created_at: String,
        archive: Option<(String, String)>,
    ) -> Self {
        Self {
            schema_version: 1,
            status: EpochStatus::Active,
            epoch,
            branch,
            snapshot: Some(crate::epoch::SnapshotInfo {
                source_branch,
                source_commit,
                commit,
                created_at,
            }),
            redirect: None,
            archive: archive.map(|(tag, sha)| crate::epoch::ArchiveInfo {
                tag,
                bundle_sha256: sha,
            }),
        }
    }

    /// Build a Redirected-state epoch file on the sealed `branch` pointing at
    /// `target_branch` (the freshly-opened next epoch).
    #[allow(clippy::too_many_arguments)]
    pub fn new_redirect(
        epoch: u32,
        branch: String,
        target_epoch: u32,
        target_branch: String,
        target_commit: String,
        snapshot_of: String,
        created_at: String,
        archive: Option<(String, String)>,
    ) -> Self {
        Self {
            schema_version: 1,
            status: EpochStatus::Redirected,
            epoch,
            branch,
            snapshot: None,
            redirect: Some(crate::epoch::RedirectInfo {
                target_epoch,
                target_branch,
                target_commit,
                snapshot_of,
                created_at,
            }),
            archive: archive.map(|(tag, sha)| crate::epoch::ArchiveInfo {
                tag,
                bundle_sha256: sha,
            }),
        }
    }

    /// Atomically write this file to `path`. Validates schema before write —
    /// callers are protected from accidentally persisting an invalid state.
    /// Implementation: write to `<path>.tmp` then `rename(.tmp, .)` so reader
    /// processes never observe a partial file.
    pub fn save_to_path(&self, path: &std::path::Path) -> Result<(), crate::epoch::EpochError> {
        self.validate()?;
        let yaml = serde_yaml::to_string(self)
            .map_err(|e| crate::epoch::EpochError::Serialize(e.to_string()))?;
        let tmp = path.with_extension("yaml.tmp");
        std::fs::write(&tmp, yaml)
            .map_err(|e| crate::epoch::EpochError::Io(e.to_string()))?;
        std::fs::rename(&tmp, path)
            .map_err(|e| crate::epoch::EpochError::Io(e.to_string()))?;
        Ok(())
    }
}
```

If `EpochError` doesn't have `Serialize` / `Io` variants, add them:

```rust
#[derive(Debug, thiserror::Error)]
pub enum EpochError {
    // ... existing variants ...
    #[error("serialize: {0}")]
    Serialize(String),
    #[error("io: {0}")]
    Io(String),
}
```

(Check existing enum first; if these names already exist with different shapes, reuse them.)

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-core --test epoch_parse 2>&1 | tail -10
```

Expected: all `epoch_parse` tests pass (6 original + 4 new = 10).

- [ ] **Step 5: cargo fmt + clippy + commit**

```bash
cargo fmt -p gitim-core
cargo clippy -p gitim-core --all-targets --no-deps --locked 2>&1 | tail -5
git add crates/gitim-core/src/epoch.rs crates/gitim-core/tests/epoch_parse.rs
git commit -m "feat(core): add EpochFile constructors + atomic save_to_path"
```

---

### Task 2: gitim-sync 加 count_commits_on_branch

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `crates/gitim-sync/tests/git_ops_test.rs`

Rotation 触发点需要"当前 epoch branch 累计 commit count"。`git rev-list --count <ref>` 直接给。包成 `GitStorage` 方法以便 mock + 单测。

- [ ] **Step 1: Write failing test**

Append to `crates/gitim-sync/tests/git_ops_test.rs`:

```rust
#[test]
fn count_commits_on_branch_returns_total_reachable_count() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(dir.path()).status().unwrap();

    // Make 3 commits.
    for i in 0..3 {
        std::fs::write(dir.path().join(format!("f{i}")), "x").unwrap();
        Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap();
        Command::new("git").args(["commit", "-m", &format!("c{i}")])
            .current_dir(dir.path()).status().unwrap();
    }

    let storage = GitStorage::new(dir.path());
    let n = storage.count_commits_on_branch("main").expect("count");
    assert_eq!(n, 3);
}

#[test]
fn count_commits_on_branch_zero_for_missing_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();

    let storage = GitStorage::new(dir.path());
    // Empty branch is not yet born — count should error or 0 (decision: error).
    let res = storage.count_commits_on_branch("nonexistent");
    assert!(res.is_err(), "missing branch must surface error, got {:?}", res);
}
```

- [ ] **Step 2: Run tests to verify failures**

```bash
cargo test -p gitim-sync --test git_ops_test count_commits 2>&1 | tail -5
```

Expected: compile error — `count_commits_on_branch` not found.

- [ ] **Step 3: Implement helper**

Append to `crates/gitim-sync/src/git.rs` inside the `impl GitStorage` block:

```rust
    /// Return the number of commits reachable from `branch` head.
    /// Equivalent to `git rev-list --count <branch>`. Missing branch → Err.
    pub fn count_commits_on_branch(&self, branch: &str) -> Result<u64, GitError> {
        let output = std::process::Command::new("git")
            .args(["rev-list", "--count", branch])
            .current_dir(&self.root)
            .output()
            .map_err(|e| GitError::Command(format!("rev-list --count: {e}")))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "rev-list --count {branch} failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        let s = String::from_utf8_lossy(&output.stdout);
        s.trim().parse::<u64>()
            .map_err(|e| GitError::Command(format!("parse count: {e}")))
    }
```

(If `GitError::Command` variant doesn't exist, reuse whichever existing variant accepts `String` for shelling-out failures — check `GitError` definition first.)

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-sync --test git_ops_test count_commits 2>&1 | tail -10
```

Expected: 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync/src/git.rs crates/gitim-sync/tests/git_ops_test.rs
git commit -m "feat(sync): add GitStorage::count_commits_on_branch"
```

---

### Task 3: gitim-sync orphan snapshot + redirect commit helpers

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `crates/gitim-sync/tests/git_ops_test.rs`

Fire 路径要在新 branch 上写 orphan commit(整个 working tree 复制成 root commit),又要在老 branch 上加一个 redirect commit(只动 `gitim.epoch.yaml`)。两个 helper,各自独立可单测。

- [ ] **Step 1: Write failing tests**

Append to `crates/gitim-sync/tests/git_ops_test.rs`:

```rust
#[test]
fn create_orphan_commit_produces_root_commit_on_new_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(dir.path()).status().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(dir.path()).status().unwrap();

    let storage = GitStorage::new(dir.path());
    let sha = storage.create_orphan_commit(
        "main-epoch-2",
        "gitim.epoch.yaml",
        "schema_version: 1\nstatus: active\nepoch: 2\nbranch: main-epoch-2\n",
        "snapshot: epoch 2 from main",
        ("daemon", "daemon@gitim"),
    ).expect("orphan");
    assert!(!sha.is_empty(), "orphan commit must return sha");

    // Verify branch exists and is a root commit (no parents).
    let parents = std::process::Command::new("git")
        .args(["rev-list", "--parents", "-n", "1", "main-epoch-2"])
        .current_dir(dir.path())
        .output().unwrap();
    let parents_str = String::from_utf8_lossy(&parents.stdout);
    // Format: "<commit_sha>\n" with no parent shas after.
    let parts: Vec<&str> = parents_str.trim().split_whitespace().collect();
    assert_eq!(parts.len(), 1, "orphan must have zero parents, got {:?}", parts);

    // Verify the new file is in the orphan tree (and only it — orphan must
    // carry the full working tree snapshot, but for this test we asserted via
    // the explicit-file path so a.txt should be there from working tree).
    let ls = std::process::Command::new("git")
        .args(["ls-tree", "-r", "--name-only", "main-epoch-2"])
        .current_dir(dir.path()).output().unwrap();
    let names: Vec<String> = String::from_utf8_lossy(&ls.stdout)
        .lines().map(|s| s.to_string()).collect();
    assert!(names.contains(&"gitim.epoch.yaml".to_string()));
    assert!(names.contains(&"a.txt".to_string()),
        "orphan must include existing working tree files, got {:?}", names);
}

#[test]
fn write_redirect_commit_appends_to_current_branch() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(dir.path()).status().unwrap();
    std::fs::write(dir.path().join("a.txt"), "hello").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(dir.path()).status().unwrap();

    let storage = GitStorage::new(dir.path());
    let sha = storage.write_redirect_commit(
        "gitim.epoch.yaml",
        "schema_version: 1\nstatus: redirected\nepoch: 1\nbranch: main\n",
        "redirect: seal epoch 1",
        ("daemon", "daemon@gitim"),
    ).expect("redirect");
    assert!(!sha.is_empty());

    // Verify main is one commit ahead of the previous tip.
    let log = Command::new("git").args(["log", "--format=%H", "main"])
        .current_dir(dir.path()).output().unwrap();
    let commits: Vec<&str> = std::str::from_utf8(&log.stdout).unwrap().trim().lines().collect();
    assert_eq!(commits.len(), 2, "expected 2 commits on main, got {:?}", commits);
    assert_eq!(commits[0], sha.trim());
}
```

- [ ] **Step 2: Run tests to verify failures**

```bash
cargo test -p gitim-sync --test git_ops_test create_orphan_commit 2>&1 | tail -5
cargo test -p gitim-sync --test git_ops_test write_redirect_commit 2>&1 | tail -5
```

Expected: compile errors — helpers not found.

- [ ] **Step 3: Implement helpers**

Append to `crates/gitim-sync/src/git.rs` inside `impl GitStorage`:

```rust
    /// Create an orphan commit on `new_branch` whose tree is the current
    /// working-tree HEAD's tree, with `epoch_yaml_path` overwritten by
    /// `epoch_yaml_content`. Returns the new commit's SHA. The branch ref
    /// is created (or updated, if pre-existing — caller must guarantee it
    /// does not exist for first-fire semantics).
    ///
    /// Implementation:
    /// 1. Write epoch.yaml to working tree.
    /// 2. `git add` it (touches index only).
    /// 3. `git write-tree` to produce new tree object including epoch.yaml.
    /// 4. `git commit-tree <tree> -m <msg>` with no parent → orphan commit.
    /// 5. `git update-ref refs/heads/<new_branch> <sha>`.
    /// 6. Reset index back to HEAD's tree (epoch.yaml from working tree is
    ///    captured in the orphan; the OLD branch should not see it yet —
    ///    `write_redirect_commit` will do its own modification).
    pub fn create_orphan_commit(
        &self,
        new_branch: &str,
        epoch_yaml_path: &str,
        epoch_yaml_content: &str,
        message: &str,
        author: (&str, &str),
    ) -> Result<String, GitError> {
        // 1. Write epoch.yaml content to working tree.
        let yaml_path = self.root.join(epoch_yaml_path);
        std::fs::write(&yaml_path, epoch_yaml_content)
            .map_err(|e| GitError::Command(format!("write epoch.yaml: {e}")))?;

        // 2. Stage.
        self.run_git(&["add", epoch_yaml_path])?;

        // 3. Write-tree.
        let tree = self.run_git_capture(&["write-tree"])?;
        let tree = tree.trim().to_string();

        // 4. Commit-tree (orphan — no -p flag).
        let (name, email) = author;
        let commit = self.run_git_capture_with_env(
            &["commit-tree", &tree, "-m", message],
            &[
                ("GIT_AUTHOR_NAME", name),
                ("GIT_AUTHOR_EMAIL", email),
                ("GIT_COMMITTER_NAME", name),
                ("GIT_COMMITTER_EMAIL", email),
            ],
        )?;
        let commit = commit.trim().to_string();

        // 5. Update ref.
        self.run_git(&["update-ref", &format!("refs/heads/{new_branch}"), &commit])?;

        // 6. Reset index back to HEAD so the OLD branch's working tree is clean
        //    (the OLD branch will get its own redirect commit separately).
        self.run_git(&["reset", "--mixed", "HEAD"])?;
        // Also remove the epoch.yaml from working tree if it didn't exist before —
        // checkout HEAD restores the original state.
        self.run_git(&["checkout", "HEAD", "--", epoch_yaml_path]).ok();
        // If checkout failed (file didn't exist in HEAD), just delete it.
        let _ = std::fs::remove_file(&yaml_path);

        Ok(commit)
    }

    /// Append a single commit to the current branch that overwrites
    /// `epoch_yaml_path` with `epoch_yaml_content`. Returns new commit SHA.
    pub fn write_redirect_commit(
        &self,
        epoch_yaml_path: &str,
        epoch_yaml_content: &str,
        message: &str,
        author: (&str, &str),
    ) -> Result<String, GitError> {
        let yaml_path = self.root.join(epoch_yaml_path);
        std::fs::write(&yaml_path, epoch_yaml_content)
            .map_err(|e| GitError::Command(format!("write epoch.yaml: {e}")))?;
        self.run_git(&["add", epoch_yaml_path])?;

        let (name, email) = author;
        let output = std::process::Command::new("git")
            .args(["commit", "-m", message])
            .env("GIT_AUTHOR_NAME", name)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_COMMITTER_NAME", name)
            .env("GIT_COMMITTER_EMAIL", email)
            .current_dir(&self.root)
            .output()
            .map_err(|e| GitError::Command(format!("commit: {e}")))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "redirect commit failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        let sha = self.run_git_capture(&["rev-parse", "HEAD"])?;
        Ok(sha.trim().to_string())
    }
```

If `run_git` / `run_git_capture` / `run_git_capture_with_env` helpers don't already exist, factor them out. Visibility:
- `run_git` / `run_git_capture_with_env` → `fn` (module-private, only used inside `git.rs`)
- `run_git_capture` → `pub(crate)` (also used from `rotate.rs`)

Add a clean public helper too, so callers outside the crate don't reach for `run_git_capture`:

```rust
    /// Return the current branch name (`git symbolic-ref --short HEAD`).
    /// Errors on detached HEAD.
    pub fn current_branch(&self) -> Result<String, GitError> {
        let out = self.run_git_capture(&["symbolic-ref", "--short", "HEAD"])?;
        Ok(out.trim().to_string())
    }
```

Then the private helpers:

```rust
    fn run_git(&self, args: &[&str]) -> Result<(), GitError> {
        let output = std::process::Command::new("git")
            .args(args).current_dir(&self.root).output()
            .map_err(|e| GitError::Command(format!("git {}: {e}", args.join(" "))))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "git {} failed: {}", args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    fn run_git_capture(&self, args: &[&str]) -> Result<String, GitError> {
        let output = std::process::Command::new("git")
            .args(args).current_dir(&self.root).output()
            .map_err(|e| GitError::Command(format!("git {}: {e}", args.join(" "))))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "git {} failed: {}", args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn run_git_capture_with_env(
        &self, args: &[&str], envs: &[(&str, &str)],
    ) -> Result<String, GitError> {
        let mut cmd = std::process::Command::new("git");
        cmd.args(args).current_dir(&self.root);
        for (k, v) in envs { cmd.env(k, v); }
        let output = cmd.output()
            .map_err(|e| GitError::Command(format!("git {}: {e}", args.join(" "))))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "git {} failed: {}", args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }
```

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-sync --test git_ops_test create_orphan_commit write_redirect_commit 2>&1 | tail -10
```

Expected: 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync/src/git.rs crates/gitim-sync/tests/git_ops_test.rs
git commit -m "feat(sync): add create_orphan_commit + write_redirect_commit helpers"
```

---

### Task 4: gitim-sync atomic_push + bundle + tag helpers

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `crates/gitim-sync/tests/git_ops_test.rs`

Fire 路径需要把两个 ref(新 branch + 老 branch redirect)原子推。bundle helper 把 sealed branch 落本地 archive。tag helper 打 `archive/epoch-N/<short>`。

- [ ] **Step 1: Write failing tests**

Append to `crates/gitim-sync/tests/git_ops_test.rs`:

```rust
fn make_bare_and_clone_with_main_commit() -> (tempfile::TempDir, tempfile::TempDir) {
    use std::process::Command;
    let bare = tempfile::TempDir::new().unwrap();
    let clone = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "--bare", "-b", "main"]).current_dir(bare.path()).status().unwrap();
    Command::new("git").args(["clone", bare.path().to_str().unwrap(), "."])
        .current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(clone.path()).status().unwrap();
    std::fs::write(clone.path().join("a.txt"), "hello").unwrap();
    Command::new("git").args(["add", "."]).current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["push", "-u", "origin", "main"]).current_dir(clone.path()).status().unwrap();
    (bare, clone)
}

#[test]
fn atomic_push_two_refs_succeeds_when_both_fast_forward() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let (_bare, clone) = make_bare_and_clone_with_main_commit();
    let storage = GitStorage::new(clone.path());

    // Build redirect commit on main + orphan on main-epoch-2.
    storage.write_redirect_commit(
        "gitim.epoch.yaml",
        "schema_version: 1\nstatus: redirected\nepoch: 1\nbranch: main\n",
        "seal epoch 1",
        ("d", "d@g"),
    ).unwrap();
    storage.create_orphan_commit(
        "main-epoch-2",
        "gitim.epoch.yaml",
        "schema_version: 1\nstatus: active\nepoch: 2\nbranch: main-epoch-2\n",
        "snapshot epoch 2",
        ("d", "d@g"),
    ).unwrap();

    storage.atomic_push_two_refs("main", "main-epoch-2").expect("atomic push ok");

    // Verify both refs landed.
    let remote_refs = Command::new("git").args(["ls-remote", "origin"])
        .current_dir(clone.path()).output().unwrap();
    let s = String::from_utf8_lossy(&remote_refs.stdout);
    assert!(s.contains("refs/heads/main\n") || s.contains("refs/heads/main\t"));
    assert!(s.contains("refs/heads/main-epoch-2"));
}

#[test]
fn atomic_push_two_refs_rejects_atomically_on_non_fast_forward() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    // Two clones share one bare. Clone A pushes a rotation first. Clone B
    // (still pointed at the pre-rotation main) tries its own rotation — server
    // rejects it atomically (main is non-fast-forward).
    let bare = tempfile::TempDir::new().unwrap();
    let clone_a = tempfile::TempDir::new().unwrap();
    let clone_b = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "--bare", "-b", "main"])
        .current_dir(bare.path()).status().unwrap();
    for cl in [&clone_a, &clone_b] {
        Command::new("git").args(["clone", bare.path().to_str().unwrap(), "."])
            .current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"])
            .current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.name", "t"])
            .current_dir(cl.path()).status().unwrap();
    }
    // Seed: A pushes one commit, B fetches.
    std::fs::write(clone_a.path().join("a.txt"), "hello").unwrap();
    Command::new("git").args(["add", "."]).current_dir(clone_a.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(clone_a.path()).status().unwrap();
    Command::new("git").args(["push", "-u", "origin", "main"]).current_dir(clone_a.path()).status().unwrap();
    Command::new("git").args(["fetch", "origin"]).current_dir(clone_b.path()).status().unwrap();
    Command::new("git").args(["reset", "--hard", "origin/main"]).current_dir(clone_b.path()).status().unwrap();

    let storage_a = GitStorage::new(clone_a.path());
    let storage_b = GitStorage::new(clone_b.path());

    // A wins the rotation push.
    storage_a.write_redirect_commit(
        "gitim.epoch.yaml", "a-redirect", "A redirect", ("a", "a@g"),
    ).unwrap();
    storage_a.create_orphan_commit(
        "main-epoch-2", "gitim.epoch.yaml", "a-orphan", "A orphan", ("a", "a@g"),
    ).unwrap();
    storage_a.atomic_push_two_refs("main", "main-epoch-2").unwrap();

    // B tries its own rotation without fetching A's redirect — must reject.
    storage_b.write_redirect_commit(
        "gitim.epoch.yaml", "b-redirect", "B redirect", ("b", "b@g"),
    ).unwrap();
    storage_b.create_orphan_commit(
        "main-epoch-2", "gitim.epoch.yaml", "b-orphan", "B orphan", ("b", "b@g"),
    ).unwrap();
    let res = storage_b.atomic_push_two_refs("main", "main-epoch-2");
    assert!(res.is_err(), "non-fast-forward must reject; got {:?}", res);

    // Verify origin/main still points to A's redirect, not B's.
    let remote_main = Command::new("git").args(["ls-remote", "origin", "main"])
        .current_dir(clone_b.path()).output().unwrap();
    let s = String::from_utf8_lossy(&remote_main.stdout);
    // Sanity: ls-remote returned non-empty (server reachable).
    assert!(!s.is_empty(), "ls-remote main empty");
}

#[test]
fn bundle_to_path_creates_readable_bundle() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(dir.path()).status().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(dir.path()).status().unwrap();

    let storage = GitStorage::new(dir.path());
    let bundle_path = dir.path().join("epoch-1.bundle");
    storage.bundle_to_path(&bundle_path, "main").expect("bundle");
    assert!(bundle_path.exists() && bundle_path.metadata().unwrap().len() > 0);

    // Verify bundle is well-formed by listing its heads.
    let out = Command::new("git").args(["bundle", "list-heads", bundle_path.to_str().unwrap()])
        .current_dir(dir.path()).output().unwrap();
    assert!(out.status.success(), "bundle list-heads should succeed");
    assert!(String::from_utf8_lossy(&out.stdout).contains("refs/heads/main"));
}

#[test]
fn tag_archive_creates_named_tag_on_ref() {
    use gitim_sync::git::GitStorage;
    use std::process::Command;

    let dir = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "-b", "main"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(dir.path()).status().unwrap();
    std::fs::write(dir.path().join("a.txt"), "x").unwrap();
    Command::new("git").args(["add", "."]).current_dir(dir.path()).status().unwrap();
    Command::new("git").args(["commit", "-m", "init"]).current_dir(dir.path()).status().unwrap();

    let storage = GitStorage::new(dir.path());
    let head_sha = storage.rev_parse("HEAD").unwrap();
    let short = &head_sha[..7];
    let tag_name = format!("archive/epoch-1/{short}");

    storage.tag_archive(&tag_name, "HEAD").expect("tag");

    let out = Command::new("git").args(["tag", "-l", &tag_name])
        .current_dir(dir.path()).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains(&tag_name));
}
```

> **Note:** the non-fast-forward test in this draft is awkward because simulating "another daemon pushed first" inside a single test requires a second clone. The inline scheme (rewind local then re-push) approximates the same git-side error. If the simulation proves brittle, fall back to a 2-clone fixture (`setup_two_clones` from `sync_e2e_test.rs`) — both clones push, second one's atomic push rejects. Plan an explicit 2-clone version in Task 7.

- [ ] **Step 2: Run tests to verify failures**

```bash
cargo test -p gitim-sync --test git_ops_test atomic_push 2>&1 | tail -5
cargo test -p gitim-sync --test git_ops_test bundle_to_path 2>&1 | tail -5
cargo test -p gitim-sync --test git_ops_test tag_archive 2>&1 | tail -5
```

Expected: compile errors — helpers not found.

- [ ] **Step 3: Implement helpers**

Append to `impl GitStorage` in `crates/gitim-sync/src/git.rs`:

```rust
    /// Atomically push two local branches to remote. Either both refs update
    /// or neither — backed by `git push --atomic`.
    pub fn atomic_push_two_refs(&self, ref_a: &str, ref_b: &str) -> Result<(), GitError> {
        let output = std::process::Command::new("git")
            .args([
                "push", "--atomic", "origin",
                &format!("{ref_a}:refs/heads/{ref_a}"),
                &format!("{ref_b}:refs/heads/{ref_b}"),
            ])
            .current_dir(&self.root)
            .output()
            .map_err(|e| GitError::Command(format!("push --atomic: {e}")))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "atomic push rejected: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Create a git bundle of `ref_name` at `bundle_path`. Parent dirs are
    /// created if missing. Best-effort: caller can ignore errors with `.ok()`.
    pub fn bundle_to_path(
        &self,
        bundle_path: &std::path::Path,
        ref_name: &str,
    ) -> Result<(), GitError> {
        if let Some(parent) = bundle_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GitError::Command(format!("mkdir bundle parent: {e}")))?;
        }
        let output = std::process::Command::new("git")
            .args(["bundle", "create", bundle_path.to_str().unwrap_or(""), ref_name])
            .current_dir(&self.root)
            .output()
            .map_err(|e| GitError::Command(format!("bundle create: {e}")))?;
        if !output.status.success() {
            return Err(GitError::Command(format!(
                "bundle create failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )));
        }
        Ok(())
    }

    /// Create lightweight tag `tag_name` pointing at `target_ref`.
    pub fn tag_archive(&self, tag_name: &str, target_ref: &str) -> Result<(), GitError> {
        self.run_git(&["tag", tag_name, target_ref])
    }
```

If `run_git_capture` is currently private and the test uses it, expose it `pub(crate)`:

```rust
    pub(crate) fn run_git_capture(&self, args: &[&str]) -> Result<String, GitError> { /* ... */ }
```

(Already `pub(crate)` from Task 3's helper definition — this is just a reminder if Task 3 was completed with stricter visibility.)

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-sync --test git_ops_test atomic_push bundle_to_path tag_archive 2>&1 | tail -15
```

Expected: all 4 new tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync/src/git.rs crates/gitim-sync/tests/git_ops_test.rs
git commit -m "feat(sync): add atomic_push_two_refs + bundle_to_path + tag_archive"
```

---

### Task 5: rotate module — try_fire_rotation (solo win path)

**Files:**
- Create: `crates/gitim-sync/src/rotate.rs`
- Modify: `crates/gitim-sync/src/lib.rs`
- Create: `crates/gitim-sync/tests/rotate_test.rs`

把 Task 2-4 的原语拼成 fire 路径。本 task **只测 solo 路径**(单 daemon,push 必赢)。race 测试在 Task 7。

- [ ] **Step 1: Write failing test**

Create `crates/gitim-sync/tests/rotate_test.rs`:

```rust
use gitim_sync::rotate::{try_fire_rotation, RotationOutcome};
use gitim_sync::git::GitStorage;
use std::process::Command;

fn setup_clone_with_n_commits(n: usize) -> (tempfile::TempDir, tempfile::TempDir) {
    let bare = tempfile::TempDir::new().unwrap();
    let clone = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "--bare", "-b", "main"]).current_dir(bare.path()).status().unwrap();
    Command::new("git").args(["clone", bare.path().to_str().unwrap(), "."])
        .current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["config", "user.email", "t@t"]).current_dir(clone.path()).status().unwrap();
    Command::new("git").args(["config", "user.name", "t"]).current_dir(clone.path()).status().unwrap();
    for i in 0..n {
        std::fs::write(clone.path().join(format!("f{i}.txt")), format!("c{i}")).unwrap();
        Command::new("git").args(["add", "."]).current_dir(clone.path()).status().unwrap();
        Command::new("git").args(["commit", "-m", &format!("c{i}")]).current_dir(clone.path()).status().unwrap();
    }
    Command::new("git").args(["push", "-u", "origin", "main"]).current_dir(clone.path()).status().unwrap();
    (bare, clone)
}

#[test]
fn try_fire_rotation_under_threshold_returns_not_ready() {
    let (_bare, clone) = setup_clone_with_n_commits(3);
    let storage = GitStorage::new(clone.path());
    let archive_dir = tempfile::TempDir::new().unwrap();

    let outcome = try_fire_rotation(
        &storage,
        "main",
        /* threshold */ 100,
        archive_dir.path(),
        ("daemon", "daemon@gitim"),
        "2026-05-21T00:00:00Z",
    ).expect("call");

    assert!(matches!(outcome, RotationOutcome::NotReady), "got {:?}", outcome);
}

#[test]
fn try_fire_rotation_over_threshold_solo_wins_and_switches_branch() {
    let (_bare, clone) = setup_clone_with_n_commits(5);
    let storage = GitStorage::new(clone.path());
    let archive_dir = tempfile::TempDir::new().unwrap();

    let outcome = try_fire_rotation(
        &storage,
        "main",
        /* threshold */ 3,
        archive_dir.path(),
        ("daemon", "daemon@gitim"),
        "2026-05-21T00:00:00Z",
    ).expect("call");

    let new_branch = match outcome {
        RotationOutcome::Won { new_branch, sealed_branch, new_epoch, .. } => {
            assert_eq!(sealed_branch, "main");
            assert_eq!(new_branch, "main-epoch-2");
            assert_eq!(new_epoch, 2);
            new_branch
        }
        other => panic!("expected Won, got {:?}", other),
    };

    // Working tree is now on the new branch.
    let head = Command::new("git").args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone.path()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), new_branch);

    // gitim.epoch.yaml on disk says active.
    let yaml = std::fs::read_to_string(clone.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active"));
    assert!(yaml.contains("epoch: 2"));

    // Bundle landed locally.
    let bundle = archive_dir.path().join("epoch-1.bundle");
    assert!(bundle.exists(), "bundle should be created at {:?}", bundle);

    // Archive tag exists.
    let tags = Command::new("git").args(["tag", "-l", "archive/epoch-1/*"])
        .current_dir(clone.path()).output().unwrap();
    assert!(!tags.stdout.is_empty(), "archive tag should exist");
}
```

- [ ] **Step 2: Run tests to verify failure**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -5
```

Expected: compile error — module doesn't exist.

- [ ] **Step 3: Implement rotate module**

Create `crates/gitim-sync/src/rotate.rs`:

```rust
//! Epoch rotation: fire path (build orphan + redirect + atomic push) and
//! follow path (checkout the freshly-published new branch). See
//! `docs/plans/git-history-snapshot-pack/02-phase-b-auto-rotate.md`.

use crate::git::{GitError, GitStorage};
use gitim_core::epoch::{EpochFile, EpochStatus};
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum RotationOutcome {
    /// Commit count under threshold; nothing to do.
    NotReady,
    /// This daemon won the push race. `new_branch` is now the local checkout.
    Won {
        sealed_branch: String,
        new_branch: String,
        new_epoch: u32,
        sealed_commit_sha: String,
        orphan_commit_sha: String,
    },
    /// Another daemon won. Caller should run `follow_redirect` to catch up.
    Lost,
}

#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    #[error("git: {0}")]
    Git(#[from] GitError),
    #[error("epoch: {0}")]
    Epoch(String),
    #[error("io: {0}")]
    Io(String),
}

/// Attempt to fire an epoch rotation on `current_branch`. Caller must hold
/// `commit_lock` for the duration of this call.
///
/// `threshold`: commit count above which rotation is attempted. Set to
///   `1_000_000` in production via the daemon-side constant.
/// `archive_dir`: where to place `epoch-{N}.bundle` on win.
/// `author`: `(name, email)` stamped on both redirect and orphan commits.
/// `created_at`: ISO-8601 timestamp embedded in epoch.yaml; caller supplies
///   to keep this function pure / testable.
pub fn try_fire_rotation(
    storage: &GitStorage,
    current_branch: &str,
    threshold: u64,
    archive_dir: &Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, RotationError> {
    // 1. Count under threshold? Bail.
    let n = storage.count_commits_on_branch(current_branch)?;
    if n < threshold {
        return Ok(RotationOutcome::NotReady);
    }

    // 2. Fetch to know whether someone else already started rotation.
    //    A best-effort fetch — if it fails (offline), we'll still attempt
    //    push and let the push reject sort it out.
    let _ = storage.fetch();

    // 3. Read current epoch from epoch.yaml on disk (or default to 1 for
    //    legacy `main`).
    let epoch_yaml_path = storage.root().join("gitim.epoch.yaml");
    let current_epoch_file = EpochFile::load_from_path(&epoch_yaml_path)
        .map_err(|e| RotationError::Epoch(format!("load epoch.yaml: {e}")))?;
    let current_epoch = current_epoch_file.as_ref().map(|f| f.epoch).unwrap_or(1);
    let new_epoch = current_epoch + 1;
    let new_branch = format!("main-epoch-{new_epoch}");

    // 4. If current branch already redirected, we shouldn't even be here —
    //    caller's follow_redirect should have caught it. Defensive bail.
    if let Some(ref ef) = current_epoch_file {
        if ef.status == EpochStatus::Redirected {
            return Ok(RotationOutcome::Lost);
        }
    }

    // 5. Capture the soon-to-be-sealed commit SHA before any writes.
    let sealed_commit_sha = storage
        .rev_parse(current_branch)
        .map_err(RotationError::Git)?
        .trim()
        .to_string();
    let sealed_short = &sealed_commit_sha[..7];
    let archive_tag = format!("archive/epoch-{current_epoch}/{sealed_short}");

    // 6. Build the new active epoch.yaml for the orphan.
    //    v1: `archive` block is None (we don't compute bundle SHA upfront, and
    //    the orphan commit SHA isn't known until after commit-tree). The
    //    `snapshot.commit` field is set to the sealed source SHA — it's
    //    informational and matches the source branch tip at the moment of
    //    rotation. Phase C (or follow-up) can patch the YAML post-orphan to
    //    embed the actual orphan SHA + bundle SHA.
    let active = EpochFile::new_active(
        new_epoch,
        new_branch.clone(),
        current_branch.to_string(),
        sealed_commit_sha.clone(),
        sealed_commit_sha.clone(),
        created_at.to_string(),
        None,
    );
    let active_yaml = serde_yaml::to_string(&active)
        .map_err(|e| RotationError::Epoch(format!("serialize active: {e}")))?;

    // 7. Build redirect epoch.yaml for the sealed branch.
    //    v1: same simplifications as active above. `redirect.target_commit`
    //    will be patched post-orphan in a later phase if needed; for now it's
    //    set to sealed_commit_sha as an informational anchor.
    let redirect = EpochFile::new_redirect(
        current_epoch,
        current_branch.to_string(),
        new_epoch,
        new_branch.clone(),
        sealed_commit_sha.clone(),
        sealed_commit_sha.clone(),
        created_at.to_string(),
        None,
    );
    let redirect_yaml = serde_yaml::to_string(&redirect)
        .map_err(|e| RotationError::Epoch(format!("serialize redirect: {e}")))?;

    // 8. Create orphan commit on new branch (carries active.yaml).
    let orphan_commit_sha = storage.create_orphan_commit(
        &new_branch,
        "gitim.epoch.yaml",
        &active_yaml,
        &format!("snapshot: open epoch {new_epoch} from {current_branch}@{sealed_short}"),
        author,
    )?;

    // 9. Write redirect commit on current branch.
    storage.write_redirect_commit(
        "gitim.epoch.yaml",
        &redirect_yaml,
        &format!("seal: redirect epoch {current_epoch} -> {new_branch}@{}", &orphan_commit_sha[..7]),
        author,
    )?;

    // 10. Atomic push both refs.
    match storage.atomic_push_two_refs(current_branch, &new_branch) {
        Ok(()) => {
            // 11. We won. Local switch to new branch.
            //     reset current_branch's local working tree first (we have
            //     epoch.yaml: redirected on disk from the write_redirect_commit
            //     step; checkout will overwrite it with active version).
            storage.run_checkout_branch(&new_branch)?;

            // 12. Best-effort: tag archive on sealed branch, push tag, write bundle.
            let _ = storage.tag_archive(&archive_tag, &sealed_commit_sha);
            let _ = storage.push_tag(&archive_tag);
            let bundle_path = archive_dir.join(format!("epoch-{current_epoch}.bundle"));
            if let Err(e) = storage.bundle_to_path(&bundle_path, &archive_tag) {
                tracing::warn!("bundle create failed (non-fatal): {}", e);
            }

            Ok(RotationOutcome::Won {
                sealed_branch: current_branch.to_string(),
                new_branch,
                new_epoch,
                sealed_commit_sha,
                orphan_commit_sha,
            })
        }
        Err(_) => {
            // Lost. Caller runs follow_redirect.
            Ok(RotationOutcome::Lost)
        }
    }
}

/// Helper used by follow_redirect (Task 6) — exposed in module so future
/// caller signatures stay stable.
pub(crate) fn archive_bundle_path(archive_dir: &Path, epoch: u32) -> PathBuf {
    archive_dir.join(format!("epoch-{epoch}.bundle"))
}
```

Add to `crates/gitim-sync/src/lib.rs`:

```rust
pub mod rotate;
```

(Alphabetical position — between existing modules.)

Add `run_checkout_branch` + `push_tag` helpers to `impl GitStorage`:

```rust
    pub fn run_checkout_branch(&self, branch: &str) -> Result<(), GitError> {
        // -f: force; safe because rotation holds commit_lock and we already
        // own all local writes.
        self.run_git(&["checkout", "-f", branch])
    }

    pub fn push_tag(&self, tag_name: &str) -> Result<(), GitError> {
        self.run_git(&["push", "origin", tag_name])
    }
```

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -10
```

Expected: 2 new tests pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
cargo clippy -p gitim-sync --all-targets --no-deps --locked 2>&1 | tail -5
git add crates/gitim-sync/src/rotate.rs crates/gitim-sync/src/lib.rs crates/gitim-sync/src/git.rs crates/gitim-sync/tests/rotate_test.rs
git commit -m "feat(sync): add rotate module with try_fire_rotation (solo path)"
```

---

### Task 6: follow_redirect

**Files:**
- Modify: `crates/gitim-sync/src/rotate.rs`
- Modify: `crates/gitim-sync/tests/rotate_test.rs`

Daemon 没有自己 fire 的情况下,sync_loop 在 `pull_rebase` 时会拉到别的 daemon publish 的 redirect。`on_synced` 回调里调 `follow_redirect` —— 切到新 branch。

- [ ] **Step 1: Write failing test**

Append to `crates/gitim-sync/tests/rotate_test.rs`:

```rust
use gitim_sync::rotate::follow_redirect;

#[test]
fn follow_redirect_no_op_when_not_redirected() {
    let (_bare, clone) = setup_clone_with_n_commits(2);
    let storage = GitStorage::new(clone.path());

    let acted = follow_redirect(&storage, "main").expect("follow");
    assert!(!acted, "no epoch.yaml present, should be no-op");
}

#[test]
fn follow_redirect_switches_to_target_branch() {
    // Setup: two clones share a bare. Clone A fires rotation. Clone B then
    // sync + follow.
    let bare = tempfile::TempDir::new().unwrap();
    let clone_a = tempfile::TempDir::new().unwrap();
    let clone_b = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "--bare", "-b", "main"]).current_dir(bare.path()).status().unwrap();
    for cl in [&clone_a, &clone_b] {
        Command::new("git").args(["clone", bare.path().to_str().unwrap(), "."])
            .current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(cl.path()).status().unwrap();
    }
    // Clone A commits + pushes 3, then fires rotation with threshold=3.
    for i in 0..3 {
        std::fs::write(clone_a.path().join(format!("f{i}")), "x").unwrap();
        Command::new("git").args(["add", "."]).current_dir(clone_a.path()).status().unwrap();
        Command::new("git").args(["commit", "-m", &format!("c{i}")]).current_dir(clone_a.path()).status().unwrap();
    }
    Command::new("git").args(["push", "origin", "main"]).current_dir(clone_a.path()).status().unwrap();
    let storage_a = GitStorage::new(clone_a.path());
    let archive_a = tempfile::TempDir::new().unwrap();
    let outcome = try_fire_rotation(
        &storage_a, "main", 3,
        archive_a.path(), ("a", "a@g"), "2026-05-21T00:00:00Z",
    ).unwrap();
    assert!(matches!(outcome, RotationOutcome::Won { .. }));

    // Clone B fetches latest and follows.
    let storage_b = GitStorage::new(clone_b.path());
    Command::new("git").args(["fetch", "origin"]).current_dir(clone_b.path()).status().unwrap();
    Command::new("git").args(["reset", "--hard", "origin/main"]).current_dir(clone_b.path()).status().unwrap();

    let acted = follow_redirect(&storage_b, "main").expect("follow");
    assert!(acted, "follow should have acted on redirected main");

    // Clone B's HEAD now on main-epoch-2.
    let head = Command::new("git").args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone_b.path()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main-epoch-2");
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p gitim-sync --test rotate_test follow_redirect 2>&1 | tail -5
```

Expected: compile error — `follow_redirect` not found.

- [ ] **Step 3: Implement follow_redirect**

Append to `crates/gitim-sync/src/rotate.rs`:

```rust
/// Check `gitim.epoch.yaml` on the current branch; if it says Redirected,
/// fetch + checkout the target_branch. Returns `true` if a switch was made.
/// Caller must hold `commit_lock`.
///
/// No-op cases:
/// - epoch.yaml absent (legacy or pre-rotation repo)
/// - epoch.yaml status == Active (nothing to follow)
/// - target_branch already current
pub fn follow_redirect(
    storage: &GitStorage,
    current_branch: &str,
) -> Result<bool, RotationError> {
    let epoch_yaml_path = storage.root().join("gitim.epoch.yaml");
    let epoch_file = match EpochFile::load_from_path(&epoch_yaml_path)
        .map_err(|e| RotationError::Epoch(format!("load: {e}")))?
    {
        Some(f) => f,
        None => return Ok(false),
    };
    if epoch_file.status != EpochStatus::Redirected {
        return Ok(false);
    }
    let target = epoch_file
        .redirect
        .as_ref()
        .ok_or_else(|| RotationError::Epoch("redirected but no redirect block".into()))?
        .target_branch
        .clone();

    if target == current_branch {
        return Ok(false);
    }

    // Fetch so the target_branch is locally known.
    storage.fetch()?;

    // Create or update local target_branch to origin's.
    let _ = storage.run_git_capture(&["branch", "-f", &target, &format!("origin/{target}")]);
    // (If branch already exists, -f re-points it. If not, it creates.)

    storage.run_checkout_branch(&target)?;
    Ok(true)
}
```

Note `run_git_capture` may need to be `pub(crate)`-visible from rotate.rs — adjust visibility if needed.

- [ ] **Step 4: Run tests to verify pass**

```bash
cargo test -p gitim-sync --test rotate_test follow_redirect 2>&1 | tail -10
```

Expected: 2 new tests pass (no-op + switch).

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync/src/rotate.rs crates/gitim-sync/src/git.rs crates/gitim-sync/tests/rotate_test.rs
git commit -m "feat(sync): add follow_redirect for non-fire daemon catch-up"
```

---

### Task 7: Race scenario tests (winner / loser)

**Files:**
- Modify: `crates/gitim-sync/tests/rotate_test.rs`

固化双 daemon race 行为:都 fire → 一个 Won,另一个 Lost,Lost 一方再 follow,两边收敛到同一个新 branch。

- [ ] **Step 1: Write failing test**

Append to `crates/gitim-sync/tests/rotate_test.rs`:

```rust
#[test]
fn race_two_daemons_only_one_wins_other_follows() {
    let bare = tempfile::TempDir::new().unwrap();
    let clone_a = tempfile::TempDir::new().unwrap();
    let clone_b = tempfile::TempDir::new().unwrap();
    Command::new("git").args(["init", "--bare", "-b", "main"]).current_dir(bare.path()).status().unwrap();
    for cl in [&clone_a, &clone_b] {
        Command::new("git").args(["clone", bare.path().to_str().unwrap(), "."])
            .current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.email", "t@t"]).current_dir(cl.path()).status().unwrap();
        Command::new("git").args(["config", "user.name", "t"]).current_dir(cl.path()).status().unwrap();
    }

    // Bootstrap: A pushes 3 commits.
    for i in 0..3 {
        std::fs::write(clone_a.path().join(format!("f{i}")), "x").unwrap();
        Command::new("git").args(["add", "."]).current_dir(clone_a.path()).status().unwrap();
        Command::new("git").args(["commit", "-m", &format!("c{i}")]).current_dir(clone_a.path()).status().unwrap();
    }
    Command::new("git").args(["push", "origin", "main"]).current_dir(clone_a.path()).status().unwrap();

    // B fetches up to same state.
    Command::new("git").args(["fetch", "origin"]).current_dir(clone_b.path()).status().unwrap();
    Command::new("git").args(["reset", "--hard", "origin/main"]).current_dir(clone_b.path()).status().unwrap();

    // Both A and B see threshold breached. Both fire (sequentially in test;
    // outcome is the same as concurrent since git push is serialized server-side).
    let storage_a = GitStorage::new(clone_a.path());
    let storage_b = GitStorage::new(clone_b.path());
    let arch_a = tempfile::TempDir::new().unwrap();
    let arch_b = tempfile::TempDir::new().unwrap();

    let oa = try_fire_rotation(&storage_a, "main", 3, arch_a.path(),
        ("a", "a@g"), "2026-05-21T00:00:00Z").unwrap();
    let ob = try_fire_rotation(&storage_b, "main", 3, arch_b.path(),
        ("b", "b@g"), "2026-05-21T00:00:00Z").unwrap();

    // Exactly one Won, one Lost. (A goes first → A wins.)
    assert!(matches!(oa, RotationOutcome::Won { .. }), "A should win, got {:?}", oa);
    assert!(matches!(ob, RotationOutcome::Lost), "B should lose, got {:?}", ob);

    // B follows.
    Command::new("git").args(["fetch", "origin"]).current_dir(clone_b.path()).status().unwrap();
    Command::new("git").args(["reset", "--hard", "origin/main"]).current_dir(clone_b.path()).status().unwrap();
    let acted = follow_redirect(&storage_b, "main").expect("follow");
    assert!(acted);

    // Both clones on main-epoch-2.
    for cl in [&clone_a, &clone_b] {
        let head = Command::new("git").args(["symbolic-ref", "--short", "HEAD"])
            .current_dir(cl.path()).output().unwrap();
        assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main-epoch-2",
            "clone at {:?} should be on main-epoch-2", cl.path());
    }
}
```

- [ ] **Step 2: Run test to verify**

```bash
cargo test -p gitim-sync --test rotate_test race_two_daemons 2>&1 | tail -10
```

Expected: passes if Task 5/6 implemented correctly. If A's "Won" returns Lost, double-check `atomic_push_two_refs` reject branch — it should map only non-fast-forward to error, not all errors.

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/tests/rotate_test.rs
git commit -m "test(sync): cover two-daemon race convergence"
```

---

### Task 8: gitim-daemon — wire rotation into sync_loop hooks

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs`
- Create: `crates/gitim-daemon/tests/epoch_rotation.rs`

挂钩 `on_pushed`(try fire) + `on_synced`(follow)。Threshold 用 `ROTATION_THRESHOLD` 常量 + `GITIM_ROTATION_THRESHOLD` env override 便于测试。

- [ ] **Step 1: Write failing end-to-end test**

Create `crates/gitim-daemon/tests/epoch_rotation.rs`:

```rust
//! End-to-end: daemon wired with low threshold rotates after enough writes.
//!
//! We bypass spawning a real daemon binary — the API surface
//! (`AppState::start_sync_loop_for_test`) exercises the wired callbacks. See
//! existing `epoch_gate.rs` for the same pattern.

use gitim_core::types::Config;
use gitim_daemon::state::AppState;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::broadcast;

#[tokio::test]
async fn daemon_auto_rotates_when_threshold_crossed() {
    std::env::set_var("GITIM_ROTATION_THRESHOLD", "3");

    let bare = TempDir::new().unwrap();
    let clone = TempDir::new().unwrap();
    std::process::Command::new("git")
        .args(["init", "--bare", "-b", "main"]).current_dir(bare.path()).status().unwrap();
    std::process::Command::new("git")
        .args(["clone", bare.path().to_str().unwrap(), "."])
        .current_dir(clone.path()).status().unwrap();
    std::process::Command::new("git")
        .args(["config", "user.email", "d@d"]).current_dir(clone.path()).status().unwrap();
    std::process::Command::new("git")
        .args(["config", "user.name", "d"]).current_dir(clone.path()).status().unwrap();

    // Pre-seed 3 commits and push.
    for i in 0..3 {
        std::fs::write(clone.path().join(format!("f{i}")), "x").unwrap();
        std::process::Command::new("git").args(["add", "."]).current_dir(clone.path()).status().unwrap();
        std::process::Command::new("git").args(["commit", "-m", &format!("c{i}")])
            .current_dir(clone.path()).status().unwrap();
    }
    std::process::Command::new("git").args(["push", "-u", "origin", "main"])
        .current_dir(clone.path()).status().unwrap();

    let (tx, _) = broadcast::channel(16);
    let state = Arc::new(AppState::new(
        clone.path().to_path_buf(),
        Config::default(),
        tx,
        None,
    ));

    // Run one rotation attempt manually via the same code path on_pushed uses.
    let state_for_block = state.clone();
    let outcome = tokio::task::spawn_blocking(move || state_for_block.attempt_rotation_for_test())
        .await
        .expect("join blocking task")
        .expect("attempt_rotation_for_test");
    assert!(outcome, "rotation should have fired");

    // HEAD now on main-epoch-2.
    let head = std::process::Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone.path()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main-epoch-2");

    std::env::remove_var("GITIM_ROTATION_THRESHOLD");
}
```

- [ ] **Step 2: Run test to verify failure**

```bash
cargo test -p gitim-daemon --test epoch_rotation 2>&1 | tail -5
```

Expected: compile error — `attempt_rotation_for_test` not found.

- [ ] **Step 3: Wire AppState — add constants + helper + on_pushed/on_synced hooks**

Modify `crates/gitim-daemon/src/state.rs`. Add near top:

```rust
/// Default production threshold: 1,000,000 commits per epoch.
/// Test/debug override via `GITIM_ROTATION_THRESHOLD` env.
pub const ROTATION_THRESHOLD_DEFAULT: u64 = 1_000_000;

fn rotation_threshold() -> u64 {
    std::env::var("GITIM_ROTATION_THRESHOLD")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(ROTATION_THRESHOLD_DEFAULT)
}
```

Add to `impl AppState` (near `refresh_epoch_status`):

```rust
    /// Test-only entry point exercising the same path the `on_pushed` hook
    /// runs. Returns true if a rotation was attempted and won.
    /// Sync function — git ops are blocking; callers from async contexts use
    /// `tokio::task::spawn_blocking`.
    pub fn attempt_rotation_for_test(&self) -> Result<bool, String> {
        self.try_rotate_inner().map_err(|e| e.to_string())
    }

    /// Internal: acquire commit_lock, call into gitim_sync::rotate.
    /// Sync because the entire body is blocking git shell-outs; no .await.
    fn try_rotate_inner(&self) -> Result<bool, gitim_sync::rotate::RotationError> {
        let _guard = self.commit_lock.lock().expect("commit_lock poisoned");
        let storage = gitim_sync::git::GitStorage::new(&self.repo_root);

        // Determine current branch.
        let branch = storage
            .current_branch()
            .map_err(gitim_sync::rotate::RotationError::Git)?;

        // Per-clone archive (gitignored — `.gitim/` is already excluded).
        let archive_dir = self.repo_root.join(".gitim").join("archive");
        let author = self.rotation_author();
        let created_at = chrono::Utc::now().to_rfc3339();

        let outcome = gitim_sync::rotate::try_fire_rotation(
            &storage,
            &branch,
            rotation_threshold(),
            &archive_dir,
            (author.0.as_str(), author.1.as_str()),
            &created_at,
        )?;

        match outcome {
            gitim_sync::rotate::RotationOutcome::Won { .. } => {
                self.refresh_epoch_status().ok();
                Ok(true)
            }
            gitim_sync::rotate::RotationOutcome::Lost => {
                // Drop into follow path; same outcome, different reason.
                gitim_sync::rotate::follow_redirect(&storage, &branch)?;
                self.refresh_epoch_status().ok();
                Ok(false)
            }
            gitim_sync::rotate::RotationOutcome::NotReady => Ok(false),
        }
    }

    /// (name, email) for rotation commits. Mirrors `rebase_author_state` logic.
    fn rotation_author(&self) -> (String, String) {
        let handler = self
            .me_json
            .read()
            .ok()
            .and_then(|me| me.as_ref().map(|m| m.handler.clone()))
            .unwrap_or_else(|| "daemon".to_string());
        let email = self
            .github_email
            .read()
            .ok()
            .and_then(|e| e.clone())
            .unwrap_or_else(|| format!("{handler}@gitim"));
        (handler, email)
    }
```

Wire into `on_pushed` (existing closure at line ~363) — after the existing PushResult logic, before returning:

```rust
                    // ... existing pending_push.drain + event broadcasts ...

                    // Rotation check: did this push tip us over threshold?
                    // try_rotate_inner is sync (blocking git shell-outs) — run on
                    // the blocking pool so we don't stall the async runtime.
                    let push_state_for_rotate = push_state.clone();
                    tokio::task::spawn_blocking(move || {
                        if let Err(e) = push_state_for_rotate.try_rotate_inner() {
                            tracing::warn!("rotation attempt failed: {}", e);
                        }
                    });
```

> **Note:** `push_state` is the captured `Arc<AppState>` clone in the closure. Verify it's already `Arc<AppState>` cloneable. If the closure captures it differently, restructure to clone `Arc<AppState>` once before passing into the closure.

Wire into `on_synced` (existing closure at ~415) — after `refresh_epoch_status` (line 446), add:

```rust
                    // If sync just pulled a redirect commit from remote,
                    // follow it before handlers can write to the sealed branch.
                    if synced_state.is_redirected() {
                        let _guard = synced_state.commit_lock.lock()
                            .expect("commit_lock poisoned");
                        let storage = gitim_sync::git::GitStorage::new(&synced_state.repo_root);
                        let branch = storage.current_branch().unwrap_or_default();
                        if let Err(e) = gitim_sync::rotate::follow_redirect(&storage, &branch) {
                            tracing::warn!("on_synced: follow_redirect failed: {}", e);
                        } else {
                            let _ = synced_state.refresh_epoch_status();
                        }
                    }
```

- [ ] **Step 4: Run integration test**

```bash
cargo test -p gitim-daemon --test epoch_rotation 2>&1 | tail -10
```

Expected: passes.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --no-deps --locked 2>&1 | tail -10
git add crates/gitim-daemon/src/state.rs crates/gitim-daemon/tests/epoch_rotation.rs
git commit -m "feat(daemon): wire epoch auto-rotate into sync_loop hooks"
```

---

### Task 9: gitim-runtime health metric

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`
- Modify: existing health test target (located in Step 1)

`/runtime/health` 加 `epoch_count`(每个 clone 的 `archive/epoch-*` tag 数 + 1)+ `total_commit_count`(sum over all clones + epochs)。让 user / agent 能在接近 GitHub size cap 时 grep 出来。

- [ ] **Step 1: Locate existing health endpoint + test**

```bash
grep -rn "runtime/health\|fn health\|HealthResponse" crates/gitim-runtime/ | head -10
```

Record the file paths in your scratch notes:
- Handler: typically `crates/gitim-runtime/src/http.rs::health` or routed in `routes.rs`
- Existing struct: search for `struct.*Health` or response builder
- Test: `crates/gitim-runtime/tests/*.rs` matching `health` in name

If no health test exists at all, create `crates/gitim-runtime/tests/health_epoch_metrics.rs` from scratch with the same fixture pattern other tests in the dir use (look at `crates/gitim-runtime/tests/cli_status.rs` as the closest analog — it spawns the runtime and calls HTTP).

- [ ] **Step 2: Write failing test**

Append (or create) at the test path from Step 1. The exact fixture wrapper depends on what's in `cli_status.rs`; the assertion shape is:

```rust
#[tokio::test]
async fn health_exposes_epoch_counters() {
    // Build a workspace with one clone that has 2 commits and one
    // `archive/epoch-1/<short>` tag — simulating a workspace that has
    // already rotated once.
    let workspace = tempfile::TempDir::new().unwrap();
    let clone_dir = workspace.path().join(".gitim-runtime").join("alice");
    std::fs::create_dir_all(&clone_dir).unwrap();
    std::process::Command::new("git").args(["init", "-b", "main-epoch-2"])
        .current_dir(&clone_dir).status().unwrap();
    std::process::Command::new("git").args(["config", "user.email", "t@t"])
        .current_dir(&clone_dir).status().unwrap();
    std::process::Command::new("git").args(["config", "user.name", "t"])
        .current_dir(&clone_dir).status().unwrap();
    for i in 0..2 {
        std::fs::write(clone_dir.join(format!("f{i}")), "x").unwrap();
        std::process::Command::new("git").args(["add", "."])
            .current_dir(&clone_dir).status().unwrap();
        std::process::Command::new("git").args(["commit", "-m", &format!("c{i}")])
            .current_dir(&clone_dir).status().unwrap();
    }
    std::process::Command::new("git").args(["tag", "archive/epoch-1/abcdef0", "HEAD"])
        .current_dir(&clone_dir).status().unwrap();

    // Spawn runtime against workspace (use the same helper cli_status.rs uses).
    let runtime = spawn_runtime_for_test(workspace.path()).await;
    let resp = http_get_json(&runtime, "/runtime/health").await;

    assert_eq!(
        resp.get("epoch_count").and_then(|v| v.as_u64()),
        Some(2),
        "epoch_count = archive tags (1) + current epoch (1) = 2, got {:?}",
        resp.get("epoch_count")
    );
    let total = resp.get("total_commit_count").and_then(|v| v.as_u64())
        .expect("total_commit_count must be present");
    assert!(total >= 2, "total_commit_count >= current branch commits, got {total}");
}
```

> `spawn_runtime_for_test` / `http_get_json` are pseudonyms — replace with the actual helpers used in `cli_status.rs`. If they don't exist, the runtime is started via `gitim_runtime::http::start_server` in those tests; use the same entry point.

- [ ] **Step 3: Run test to verify failure**

```bash
cargo test -p gitim-runtime health_exposes_epoch_counters 2>&1 | tail -5
```

Expected: assertion fail (or compile error if struct fields don't exist yet).

- [ ] **Step 4: Implement metric collection in health handler**

In `crates/gitim-runtime/src/http.rs`, add helper function:

```rust
fn collect_epoch_metrics(workspace_dir: &std::path::Path) -> (u64, u64) {
    let agents_root = workspace_dir.join(".gitim-runtime");
    let mut epoch_count: u64 = 0;
    let mut total_commits: u64 = 0;
    let entries = match std::fs::read_dir(&agents_root) {
        Ok(e) => e,
        Err(_) => return (0, 0),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // Must be an agent clone — has a .git dir (file or directory both count
        // for the gitlink case, but for clones it's a directory).
        if !path.is_dir() || !path.join(".git").exists() {
            continue;
        }

        // Count archive/epoch-* tags.
        let tag_output = std::process::Command::new("git")
            .args(["tag", "-l", "archive/epoch-*"])
            .current_dir(&path).output();
        let archived_epochs: u64 = match tag_output {
            Ok(o) if o.status.success() => {
                let s = String::from_utf8_lossy(&o.stdout);
                s.lines().filter(|l| !l.trim().is_empty()).count() as u64
            }
            _ => 0,
        };

        // Count commits on current HEAD.
        let count_output = std::process::Command::new("git")
            .args(["rev-list", "--count", "HEAD"])
            .current_dir(&path).output();
        let current_commits: u64 = match count_output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).trim().parse().unwrap_or(0)
            }
            _ => 0,
        };

        // Sum commits across each archived epoch tag (each is the sealed tip of
        // a former branch; rev-list counts the reachable commit history under
        // that orphan).
        let mut archived_commits: u64 = 0;
        if let Ok(o) = std::process::Command::new("git")
            .args(["tag", "-l", "archive/epoch-*"])
            .current_dir(&path).output()
        {
            for tag in String::from_utf8_lossy(&o.stdout).lines() {
                let t = tag.trim();
                if t.is_empty() { continue; }
                if let Ok(c) = std::process::Command::new("git")
                    .args(["rev-list", "--count", t])
                    .current_dir(&path).output()
                {
                    if c.status.success() {
                        archived_commits += String::from_utf8_lossy(&c.stdout)
                            .trim().parse::<u64>().unwrap_or(0);
                    }
                }
            }
        }

        // Per-clone epoch_count is archived + current. Workspace-level
        // epoch_count = max across clones (they should agree, but use max
        // as a conservative reading).
        epoch_count = epoch_count.max(archived_epochs + 1);
        total_commits = total_commits.saturating_add(current_commits + archived_commits);
    }
    (epoch_count, total_commits)
}
```

Update the health response struct (find existing definition — likely in `http.rs`):

```rust
#[derive(serde::Serialize)]
struct HealthResponse {
    // ... existing fields kept verbatim ...
    epoch_count: u64,
    total_commit_count: u64,
}
```

In the health handler, populate the new fields:

```rust
    let (epoch_count, total_commit_count) = collect_epoch_metrics(&runtime.workspace_path);
    // ... existing response build, with the two new fields ...
```

> The exact handler signature depends on the existing code shape (axum / actix / hyper). Match what's there; only add the two new fields and the helper call.

- [ ] **Step 5: Run test to verify pass**

```bash
cargo test -p gitim-runtime health_exposes_epoch_counters 2>&1 | tail -10
```

Expected: passes.

- [ ] **Step 6: Commit**

```bash
cargo fmt -p gitim-runtime
git add crates/gitim-runtime/src/http.rs crates/gitim-runtime/tests/
git commit -m "feat(runtime): expose epoch_count + total_commit_count on /runtime/health"
```

---

### Task 10: CLAUDE.md orientation + plan landing footer

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/plans/git-history-snapshot-pack/02-phase-b-auto-rotate.md` (this file — add landing footer)

文档化:Current Orientation 段加 Phase B landed,Tensions 移走 Phase A 第 (3)/(5) 条(handler TOCTOU / sticky upstream)—— Phase B 用 `commit_lock` 全程持有 + 单 winner push 兜底解决了 race window 大部分场景。

- [ ] **Step 1: Update CLAUDE.md Current Orientation**

Locate the "Current Orientation" section. Append to **Where we are**:

```markdown
**Snapshot Pack Phase B(auto-rotate)** 已落地:每个 daemon 在 sync_loop `on_pushed` 后检查当前 epoch branch commit count,过 1M(默认,`GITIM_ROTATION_THRESHOLD` 可 override)触发 `gitim_sync::rotate::try_fire_rotation` —— `commit_lock` 全程持有,fetch latest → orphan commit on `main-epoch-{N+1}`(整个 working tree snapshot + epoch.yaml:active)→ redirect commit on 当前 branch(epoch.yaml:redirected)→ `git push --atomic` 两 ref。GitHub 单 winner serialize:reject 一方走 `follow_redirect` 路径(fetch + branch -f origin/<target> + checkout)。Winner 顺手打 `archive/epoch-N/<short>` tag + 本地 bundle 落 `<repo_root>/.gitim/archive/epoch-N.bundle`(per-clone,gitignored,best-effort,失败 warn 不阻塞)。**无 replay queue**:`commit_lock` 在 fire 全程持有,handler 写阻塞期间没有 in-flight unpushed commits。**Follow** 也持 `commit_lock`,checkout 切换跟 handler 写串行,handler 醒来自动写到新 branch,**不需要 enforcement gate**(Phase A 的 25 handler `check_writable` 不再做)。AppState 在 `on_pushed` spawn rotation 尝试,在 `on_synced::refresh_epoch_status` 之后检测 `is_redirected()` 调 follow。Bundle 不上传远端(v1 scope),epoch 全留不 prune(v1 scope)。`/runtime/health` 新增 `epoch_count` + `total_commit_count`,user / agent 接近 GitHub size cap 时能看见。

Phase B plan 见 `docs/plans/git-history-snapshot-pack/02-phase-b-auto-rotate.md`。non-goals(留给 Phase C 或 future work):bundle 远端上传 / auto-prune / daemon-web TS port 并行 auto-rotate / multi-epoch 全文索引。
```

Move Phase A tensions "(3) TOCTOU 在 commit_lock 外" + "(4) handler `push_with_retry` 平行 push 路径" 到 **Learnings** 段(已通过 commit_lock 全程持有 fire + 走 sync_loop 单一触发点解决),或直接删除。

- [ ] **Step 2: Add landing footer to this plan**

Edit `docs/plans/git-history-snapshot-pack/02-phase-b-auto-rotate.md` (top of file, replace the existing front matter status):

```markdown
> **Status:** ✅ Landed YYYY-MM-DD in commit range `<first>..<last>` on branch `<branch-name>`.
>
> N 个 impl commit:<list each with one-line description>。
> Phase B 测试覆盖:gitim-sync rotate_test(M tests) + gitim-daemon epoch_rotation(K tests)。Scoped regression (`cargo test -p gitim-core -p gitim-sync -p gitim-daemon -p gitim-runtime`) X pass / 0 fail。Orientation 见 CLAUDE.md "Snapshot Pack Phase B" 段。
```

(Filled in by the agent after final commit.)

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md docs/plans/git-history-snapshot-pack/02-phase-b-auto-rotate.md
git commit -m "docs: record Snapshot Pack Phase B landing"
```

---

## Final Verification

After all 10 tasks, run full scoped regression:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --no-deps --locked 2>&1 | tail -20
cargo test -p gitim-core -p gitim-sync -p gitim-daemon -p gitim-runtime 2>&1 | tail -20
```

Expected:
- fmt clean
- clippy 0 new errors (baseline pre-existing errors in `gitim-agent-provider` documented in CLAUDE.md — should NOT touch)
- All target tests pass + new rotation tests counted

Then open PR with summary:
- Feature: epoch auto-rotate
- Threshold: 1M commits (env-overridable)
- v1 scope omits: bundle upload, auto-prune, daemon-web TS port
- Replaces spec's Phase B (manual coordinator) + Phase C (replay queue)
