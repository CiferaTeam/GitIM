# Snapshot Pack Phase B v2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` (recommended) or `superpowers:executing-plans` to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Design**: [`03-phase-b-v2-design.md`](03-phase-b-v2-design.md)（必读——协议不变量 + 竞态矩阵在那里）。
> 前版 plan [`02-phase-b-auto-rotate.md`](02-phase-b-auto-rotate.md) 的 Task 1-3 成果在
> `worktree-pr3-auto-rotate` 分支，本 plan Task 1 把它们 cherry-pick 进来。

**Goal:** Epoch 自动 rotation 的竞态完备实现——fire（atomic push 仲裁）+ follow（origin 为准、多跳）+ fence（push 前拦截）+ migrate（rebase --onto 零丢失搬运）。

**Architecture:** `gitim-sync` 加 git 原语和 `rotate` 模块；`sync_loop` 的 push 路径插 fence；daemon 在 `on_pushed`/`on_synced`/boot 三处接线；daemon-web 只做只读拦截。全程依赖三条协议不变量（见 design 文档），跨节点零协调。

**Tech Stack:** Rust（serde_yaml / tempfile / tokio spawn_blocking）、TypeScript（daemon-web）。

**测试节奏**：每个 task 只跑 scoped 测试（`cargo test -p gitim-sync --test rotate_test` 等），不跑全量。

---

## Review 修正记录（Task 1-3 quality review 后并入）

- **C1**：`atomic_push_two_refs` / `push_tag` 必须走 `classify_remote_error`（凭据脱敏 +
  错误分型），Task 4 据此把 `PushConflict` 映射 `Lost`，其他 remote 错误 cleanup 后上抛。
- **I1**：`run_git_command` 统一 `LC_ALL=C`（stderr 匹配去 locale 化）。
- **I2**：`count_commits_on_branch` / `run_git_capture_with_env` / `write_redirect_commit`
  的 env-ful commit 全部路由到带 timeout 的执行层。
- **I3**（design 已并入协议）：`try_fire_rotation` 开头加 `has_unpushed_commits` 守卫 →
  `NotReady`；`cleanup_failed_fire` reset 前验证 ahead-of-origin 全部是自产 commit。
- **I4**（design 已并入约束）：混版本 workspace 不支持，运维约束，无代码。

**rotate.rs review（第二轮，Task 4-5 后）追加修正：**

- **R-I1**：Won arm 的 set-upstream 失败重试一次 + 修正"sync 会自愈"的错误注释
  （sync_cycle 顶部 `@{upstream}` 探针 bail 在 push 之前，自愈到不了）；Task 8 的
  daemon boot 增加 upstream 校验修复。
- **R-I2**：fire 守卫扩展——`status --porcelain --untracked-files=no` 非空（dirty
  tracked files = send.rs commit 失败后留盘等 sync 捡的消息）→ `NotReady`；
  follow 无法推迟，在 follow doc 注明接受的残留窗口。
- **R-I3**：`follow_redirect` 的 migrate Err 路径先 `abort_rebase()` 再传播
  （恢复"Err ⇒ HEAD 回到原分支、消息完好"契约）。
- **R-I4**：补 Shape B migrate 测试（R 已在本地链上 → follow → 消息上新分支、
  无 seal commit 被重放、老分支对齐 origin）。
- **R-I5**：fence helper 加"HEAD redirected ∧ origin active → 重试 cleanup"自愈分支
  （已并入 Task 7 代码段）；fence 对 corrupt epoch.yaml fail-closed（同段）。
- **R-M 系列**：`[..7]` 用 `.get(..7)` 防 panic；`SEAL_SUBJECT_PREFIX` 常量提取（生产
  与校验共用）；`cleanup_failed_fire` doc 补 commit_lock 契约；`create_orphan_commit`
  的存在性注释对齐实际 clobber 语义；`migrate_unpushed` 注明 HEAD-attached 前置条件。
- **Task 8 注意**：`recover_from_stale_rebase` 的 reattach 目标走 `origin/HEAD`，
  rotation 后指向 sealed 分支——reattach 后 fence+follow 会收敛，无需改机制，但
  集成测试验证这条路径。
- **Task 8 注意 2**：boot 的残留清理（`cleanup_failed_fire`）必须在 daemon 开始
  accept handler 流量**之前**完成——清理含 `reset --hard`，若与 send.rs 的
  deferred-dirty-file（commit 失败留盘）并发会丢未提交消息。boot 串行序保证即可，
  无需额外 gate。
- **M1**：`write_redirect_commit` 用 `commit --only -- <path>`（结构性保证 R 只含 yaml flip）。
- **M5**：EpochFile 构造器测试加字段值断言。

## 竞态场景 → 测试映射（验收总表）

| Design 矩阵场景 | 测试 | Task |
|---|---|---|
| 1 fire vs fire | `race_two_daemons_only_one_wins_other_follows` | 6 |
| 2 fire 输给普通 push | `fire_loses_to_normal_push_cleans_up_and_self_heals` | 6 |
| 3 普通 push 输给 fire | `normal_push_loses_to_fire_message_migrates` | 6 |
| 4 R 之上写消息被 fence 拦 | `fence_blocks_push_when_head_redirected` | 6 |
| 6 多跳 follow | `follow_resolves_across_two_epochs` | 6 |
| 7 半成品 fire boot 清理 | `boot_cleanup_resets_partial_fire_residue` | 6 |
| 8 migrate 冲突走 renumber | `migrate_conflict_falls_back_to_renumber`（daemon 集成） | 8 |
| I3 fire 守卫（零丢失） | `fire_with_unpushed_backlog_returns_not_ready` | 6 |
| I3 清理自产验证 | `cleanup_refuses_when_foreign_commits_ahead` | 6 |

---

### Task 1: Cherry-pick 分支资产 + 适配 main 漂移

**Files:**
- Modify: `crates/gitim-core/src/epoch.rs`（构造器 + save_to_path 进来）
- Modify: `crates/gitim-core/tests/epoch_parse.rs`
- Modify: `crates/gitim-sync/src/git.rs`（count/orphan/redirect 三原语进来）
- Modify: `crates/gitim-sync/tests/git_ops_test.rs`

- [ ] **Step 1: Cherry-pick 六个 commits（按序）**

```bash
git cherry-pick e8979778 cc8a092a 18507afc 8517cb1f 134c4691 7e2bc033
```

冲突预期在 `git.rs`（main 在分支分叉后加了 `divergence_from_upstream` 等函数）。解决原则：**保留双方**——main 的新函数 + 分支的新函数共存；`use` 块合并去重。`epoch.rs` 预期干净（main 上该文件自分叉未动——若冲突同样保留双方）。

- [ ] **Step 2: 验证 cherry-pick 后测试通过**

```bash
cargo test -p gitim-core --test epoch_parse 2>&1 | tail -3
cargo test -p gitim-sync --test git_ops_test 2>&1 | tail -3
```

Expected: 全 PASS。失败则修适配（典型：`GitError` variant 演化、import path 变更）。

- [ ] **Step 3: Commit（cherry-pick 已各自成 commit，此步仅在有适配修改时）**

```bash
git add -A && git commit -m "fix(sync): adapt cherry-picked rotation primitives to current main"
```

---

### Task 2: git.rs 原语 — show_file_at_ref + atomic_push_two_refs

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Test: `crates/gitim-sync/tests/git_ops_test.rs`

- [ ] **Step 1: Write failing tests**

Append to `git_ops_test.rs`（复用文件内现有 `setup_*` helper 建 bare+clone）：

```rust
#[test]
fn show_file_at_ref_reads_committed_content_without_checkout() {
    let (_bare, clone) = setup_clone_with_commits(1); // 现有 helper，命名以文件内为准
    std::fs::write(clone.path().join("probe.txt"), "v1").unwrap();
    git(&clone, &["add", "."]);
    git(&clone, &["commit", "-m", "add probe"]);

    let storage = GitStorage::new(clone.path());
    let content = storage.show_file_at_ref("HEAD", "probe.txt").expect("call");
    assert_eq!(content.as_deref(), Some("v1"));
    // 不存在的 path → Ok(None)，不是 Err
    assert!(storage.show_file_at_ref("HEAD", "nope.txt").unwrap().is_none());
}

#[test]
fn atomic_push_two_refs_all_or_nothing() {
    // A、B 两个 clone 共享 bare。A 先正常 push 一个 commit 占住 main，
    // B 基于旧 tip 同时推 main + 新分支 → 整体 reject，新分支也不应出现在 bare。
    let (bare, clone_a, clone_b) = setup_two_clones(); // 现有/新建 helper
    commit_file(&clone_a, "fa", "wins");
    git(&clone_a, &["push", "origin", "main"]);

    // B 本地：main 上加 commit + 开新分支
    commit_file(&clone_b, "fb", "loses");
    git(&clone_b, &["branch", "feature-x"]);

    let storage_b = GitStorage::new(clone_b.path());
    let result = storage_b.atomic_push_two_refs("main", "feature-x");
    assert!(result.is_err(), "non-ff main must reject the whole atomic push");

    // bare 上不应存在 feature-x（all-or-nothing）
    let refs = std::process::Command::new("git")
        .args(["branch", "-l", "feature-x"])
        .current_dir(bare.path()).output().unwrap();
    assert!(refs.stdout.is_empty(), "feature-x must not exist on remote");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-sync --test git_ops_test show_file_at_ref 2>&1 | tail -3
```

Expected: compile error（method not found）。

- [ ] **Step 3: Implement**

`impl GitStorage`（跟随文件内 `run_git` 自由函数 + 方法包装的现有模式）：

```rust
    /// `git show <ref>:<path>` — read a file's committed content without
    /// touching the working tree. Returns Ok(None) when the path does not
    /// exist at that ref (exit code 128 with "does not exist"/"invalid
    /// object name" on stderr); other failures map to GitError.
    pub fn show_file_at_ref(
        &self,
        reference: &str,
        path: &str,
    ) -> Result<Option<String>, GitError> {
        let spec = format!("{reference}:{path}");
        match run_git(&["show", &spec], &self.root) {
            Ok(out) => Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned())),
            Err(GitError::CommandFailed(msg))
                if msg.contains("does not exist")
                    || msg.contains("invalid object name")
                    || msg.contains("exists on disk, but not in") =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// `git push --atomic origin <new>:refs/heads/<new> <old>:refs/heads/<old>`.
    /// Both refs update or neither does — this is the rotation arbiter
    /// (design invariant 2). Reject (any cause) is an Err; caller treats
    /// it as "lost the race", never retries blindly.
    pub fn atomic_push_two_refs(
        &self,
        old_branch: &str,
        new_branch: &str,
    ) -> Result<(), GitError> {
        let new_spec = format!("{new_branch}:refs/heads/{new_branch}");
        let old_spec = format!("{old_branch}:refs/heads/{old_branch}");
        run_git(
            &["push", "--atomic", "origin", &new_spec, &old_spec],
            &self.root,
        )
        .map(|_| ())
    }
```

注意：`run_git` 的错误映射如果没有 `CommandFailed(String)` 这种带 stderr 的 variant，按文件内实际 `GitError` 形态调整匹配臂——先读现有 `GitError` 定义再写匹配。

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p gitim-sync --test git_ops_test 2>&1 | tail -5
```

Expected: 新增 2 测试 PASS，存量不破。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync && git commit -m "feat(sync): add show_file_at_ref + atomic_push_two_refs primitives"
```

---

### Task 3: git.rs 原语 — migrate / 清理 / 归档类

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Test: `crates/gitim-sync/tests/git_ops_test.rs`

- [ ] **Step 1: Write failing tests**

```rust
#[test]
fn rebase_onto_moves_unpushed_commits_to_new_base() {
    // main: c0 推到 origin；本地再加 m1。新分支 nb 从 c0 开出加 b1 并推。
    // rebase_onto("origin/nb", "origin/main") 后：HEAD 链 = b1 → m1'，m1' 内容保留。
    let (_bare, clone) = setup_clone_with_commits(1);
    git(&clone, &["push", "origin", "main"]);
    commit_file(&clone, "m1.txt", "local msg");

    git(&clone, &["branch", "nb", "origin/main"]);
    git(&clone, &["checkout", "nb"]);
    commit_file(&clone, "b1.txt", "on new base");
    git(&clone, &["push", "origin", "nb"]);
    git(&clone, &["checkout", "main"]);

    let storage = GitStorage::new(clone.path());
    storage.rebase_onto("origin/nb", "origin/main").expect("rebase");

    assert!(clone.path().join("b1.txt").exists(), "new base content present");
    assert!(clone.path().join("m1.txt").exists(), "migrated commit content present");
}

#[test]
fn reset_branch_and_delete_branch_cleanup() {
    let (_bare, clone) = setup_clone_with_commits(1);
    git(&clone, &["push", "origin", "main"]);
    commit_file(&clone, "residue.txt", "local-only");
    git(&clone, &["branch", "stale-orphan"]);

    let storage = GitStorage::new(clone.path());
    storage.reset_branch_to_origin("main").expect("reset");
    assert!(!clone.path().join("residue.txt").exists());
    storage.delete_local_branch("stale-orphan").expect("delete");
    let out = std::process::Command::new("git").args(["branch", "-l", "stale-orphan"])
        .current_dir(clone.path()).output().unwrap();
    assert!(out.stdout.is_empty());
}

#[test]
fn tag_archive_bundle_roundtrip() {
    let (_bare, clone) = setup_clone_with_commits(2);
    git(&clone, &["push", "origin", "main"]);
    let storage = GitStorage::new(clone.path());
    let sha = storage.rev_parse("HEAD").unwrap().trim().to_string();

    storage.tag_archive("archive/epoch-1/abc1234", &sha).expect("tag");
    storage.push_tag("archive/epoch-1/abc1234").expect("push tag");

    let dir = tempfile::TempDir::new().unwrap();
    let bundle = dir.path().join("epoch-1.bundle");
    storage.bundle_to_path(&bundle, "archive/epoch-1/abc1234").expect("bundle");
    assert!(bundle.exists());
    // bundle 可被 git 验证
    let verify = std::process::Command::new("git")
        .args(["bundle", "verify", bundle.to_str().unwrap()])
        .current_dir(clone.path()).status().unwrap();
    assert!(verify.success());
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-sync --test git_ops_test rebase_onto 2>&1 | tail -3
```

Expected: compile error。

- [ ] **Step 3: Implement**

```rust
    /// `git rebase --onto <new_base> <old_base>` — transplant the commits in
    /// `<old_base>..HEAD` onto `<new_base>`. The migrate primitive (design
    /// scenario 3/4): snapshot carries the full tree, so thread appends apply
    /// cleanly; conflicts surface as Err and the caller falls back to the
    /// capture-and-replay path.
    pub fn rebase_onto(&self, new_base: &str, old_base: &str) -> Result<(), GitError> {
        run_git(&["rebase", "--onto", new_base, old_base], &self.root).map(|_| ())
    }

    /// `git checkout -f <branch> && git reset --hard origin/<branch>` 的组合语义：
    /// 把本地分支强制对齐 origin。Lost/crash 清理用（design 场景 1/2/7）。
    pub fn reset_branch_to_origin(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["checkout", "-f", branch], &self.root)?;
        let origin_ref = format!("origin/{branch}");
        run_git(&["reset", "--hard", &origin_ref], &self.root).map(|_| ())
    }

    pub fn delete_local_branch(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["branch", "-D", branch], &self.root).map(|_| ())
    }

    pub fn checkout_branch(&self, branch: &str) -> Result<(), GitError> {
        // -f: rotation holds commit_lock; any dirty state is crash residue
        // and git history is the source of truth.
        run_git(&["checkout", "-f", branch], &self.root).map(|_| ())
    }

    pub fn tag_archive(&self, tag: &str, sha: &str) -> Result<(), GitError> {
        run_git(&["tag", tag, sha], &self.root).map(|_| ())
    }

    pub fn push_tag(&self, tag: &str) -> Result<(), GitError> {
        run_git(&["push", "origin", tag], &self.root).map(|_| ())
    }

    pub fn bundle_to_path(&self, path: &Path, reference: &str) -> Result<(), GitError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| GitError::Io(e.to_string()))?; // 按现有 GitError IO variant 调整
        }
        let p = path.to_string_lossy();
        run_git(&["bundle", "create", &p, reference], &self.root).map(|_| ())
    }
```

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p gitim-sync --test git_ops_test 2>&1 | tail -5
```

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync && git commit -m "feat(sync): add migrate/cleanup/archive git primitives"
```

---

### Task 4: rotate.rs — try_fire_rotation（origin 判定 + Lost 清理）

**Files:**
- Create: `crates/gitim-sync/src/rotate.rs`
- Modify: `crates/gitim-sync/src/lib.rs`（`pub mod rotate;`）
- Create: `crates/gitim-sync/tests/rotate_test.rs`

- [ ] **Step 1: Write failing tests**

```rust
use gitim_sync::git::GitStorage;
use gitim_sync::rotate::{try_fire_rotation, RotationOutcome};
use std::process::Command;

// === helpers（本文件后续 task 共用）===
fn git(dir: &tempfile::TempDir, args: &[&str]) {
    assert!(Command::new("git").args(args).current_dir(dir.path()).status().unwrap().success());
}
fn commit_file(dir: &tempfile::TempDir, name: &str, content: &str) {
    std::fs::write(dir.path().join(name), content).unwrap();
    git(dir, &["add", "."]);
    git(dir, &["commit", "-m", name]);
}
fn setup_bare_and_clone(n_commits: usize) -> (tempfile::TempDir, tempfile::TempDir) {
    let bare = tempfile::TempDir::new().unwrap();
    let clone = tempfile::TempDir::new().unwrap();
    git(&bare, &["init", "--bare", "-b", "main"]);
    git(&clone, &["clone", bare.path().to_str().unwrap(), "."]);
    git(&clone, &["config", "user.email", "t@t"]);
    git(&clone, &["config", "user.name", "t"]);
    for i in 0..n_commits {
        commit_file(&clone, &format!("f{i}.txt"), &format!("c{i}"));
    }
    git(&clone, &["push", "-u", "origin", "main"]);
    (bare, clone)
}
fn clone_from(bare: &tempfile::TempDir) -> tempfile::TempDir {
    let c = tempfile::TempDir::new().unwrap();
    git(&c, &["clone", bare.path().to_str().unwrap(), "."]);
    git(&c, &["config", "user.email", "t@t"]);
    git(&c, &["config", "user.name", "t"]);
    c
}
fn head_branch(dir: &tempfile::TempDir) -> String {
    let out = Command::new("git").args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir.path()).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn under_threshold_returns_not_ready() {
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(&storage, "main", 100, arch.path(),
        ("d", "d@g"), "2026-06-10T00:00:00Z").unwrap();
    assert!(matches!(o, RotationOutcome::NotReady));
}

#[test]
fn solo_fire_wins_switches_branch_tags_and_bundles() {
    let (_bare, clone) = setup_bare_and_clone(5);
    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(&storage, "main", 3, arch.path(),
        ("d", "d@g"), "2026-06-10T00:00:00Z").unwrap();
    let RotationOutcome::Won { new_branch, new_epoch, sealed_branch, .. } = o else {
        panic!("expected Won, got {o:?}");
    };
    assert_eq!((sealed_branch.as_str(), new_branch.as_str(), new_epoch), ("main", "main-epoch-2", 2));
    assert_eq!(head_branch(&clone), "main-epoch-2");
    let yaml = std::fs::read_to_string(clone.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active") && yaml.contains("epoch: 2"));
    assert!(arch.path().join("epoch-1.bundle").exists());
}

#[test]
fn fire_loses_to_normal_push_cleans_up_and_self_heals() {
    // 设计矩阵场景 2：fire 期间别人先推了普通消息 → atomic reject →
    // 本地清理干净（无 redirect 残留、无废 orphan ref）、origin 无 rotation。
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

    // B 先占 main：普通 push 一个新 commit。
    commit_file(&clone_b, "msg.txt", "normal write wins");
    git(&clone_b, &["push", "origin", "main"]);

    // A 不知情（不 fetch 的状态由 try_fire 内部 fetch 决定——它 fetch 后会看到
    // origin 已前进，但 A 的本地 main 还在旧 tip，atomic push 必 reject）。
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(&storage_a, "main", 3, arch.path(),
        ("a", "a@g"), "2026-06-10T00:00:00Z").unwrap();
    assert!(matches!(o, RotationOutcome::Lost), "got {o:?}");

    // 清理断言：HEAD 还在 main；工作树 epoch.yaml 不存在（origin 无 rotation）；
    // 本地无 main-epoch-2 残留 ref；本地 main == origin/main。
    assert_eq!(head_branch(&clone_a), "main");
    assert!(!clone_a.path().join("gitim.epoch.yaml").exists());
    let out = Command::new("git").args(["branch", "-l", "main-epoch-2"])
        .current_dir(clone_a.path()).output().unwrap();
    assert!(out.stdout.is_empty(), "stale orphan branch must be deleted");
    let local = storage_a.rev_parse("main").unwrap();
    let remote = storage_a.rev_parse("origin/main").unwrap();
    assert_eq!(local, remote, "local main must be reset to origin");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -3
```

Expected: compile error — module `rotate` not found。

- [ ] **Step 3: Implement rotate.rs（fire 部分）**

```rust
//! Epoch rotation: fire (orphan + redirect + atomic push arbitration),
//! follow (origin-authoritative, multi-hop), fence (pre-push guard) and
//! migrate (rebase --onto). Protocol invariants + race matrix:
//! docs/plans/git-history-snapshot-pack/03-phase-b-v2-design.md

use crate::git::{GitError, GitStorage};
use gitim_core::epoch::{EpochFile, EpochStatus};
use std::path::Path;

pub const EPOCH_FILE: &str = "gitim.epoch.yaml";
/// Multi-hop follow guard (design scenario 6). 32 is unreachable in
/// practice — it exists to turn a metadata cycle into an error, not a hang.
pub const MAX_FOLLOW_HOPS: u32 = 32;

#[derive(Debug)]
pub enum RotationOutcome {
    NotReady,
    Won {
        sealed_branch: String,
        new_branch: String,
        new_epoch: u32,
        sealed_commit_sha: String,
        orphan_commit_sha: String,
    },
    Lost,
}

#[derive(Debug, thiserror::Error)]
pub enum RotationError {
    #[error("git: {0}")]
    Git(#[from] GitError),
    #[error("epoch: {0}")]
    Epoch(String),
}

/// Parse `gitim.epoch.yaml` as committed at `<ref>` (not the working tree —
/// design invariant 3: decisions trust origin, never local residue).
fn epoch_at_ref(
    storage: &GitStorage,
    reference: &str,
) -> Result<Option<EpochFile>, RotationError> {
    let Some(content) = storage.show_file_at_ref(reference, EPOCH_FILE)? else {
        return Ok(None);
    };
    let file: EpochFile = serde_yaml::from_str(&content)
        .map_err(|e| RotationError::Epoch(format!("parse {reference}:{EPOCH_FILE}: {e}")))?;
    file.validate()
        .map_err(|e| RotationError::Epoch(format!("validate {reference}:{EPOCH_FILE}: {e}")))?;
    Ok(Some(file))
}

/// Remove every local trace of a failed fire so the next cycle starts
/// clean: reset old branch onto origin, drop the never-published orphan
/// branch. Also the boot-time cleanup for crash residue (scenario 7).
///
/// Zero-loss guard (review I3): reset only when everything ahead of origin
/// is rotation-self-produced. A foreign commit in that range means messages
/// would die — leave the residue in place (the push fence keeps it
/// unpublished; delayed, never lost) and let a human look.
pub fn cleanup_failed_fire(
    storage: &GitStorage,
    old_branch: &str,
    orphan_branch: &str,
) -> Result<(), RotationError> {
    let ahead = storage.subjects_ahead_of_origin(old_branch)?;
    if ahead.iter().any(|s| !s.starts_with("seal: redirect")) {
        tracing::warn!(
            "cleanup_failed_fire: non-rotation commits ahead of origin/{old_branch} \
             ({ahead:?}); refusing to reset — residue stays fenced until resolved"
        );
        return Ok(());
    }
    storage.reset_branch_to_origin(old_branch)?;
    // Branch may not exist if we crashed before creating it — best-effort.
    let _ = storage.delete_local_branch(orphan_branch);
    Ok(())
}
```

需要一个配套小原语（`git.rs`）：

```rust
    /// Subjects of commits in `origin/<branch>..<branch>` (oldest first).
    /// Empty when the branch is in sync with origin.
    pub fn subjects_ahead_of_origin(&self, branch: &str) -> Result<Vec<String>, GitError> {
        let range = format!("origin/{branch}..{branch}");
        let out = run_git(&["log", "--reverse", "--format=%s", &range], &self.root)?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect())
    }

/// Attempt to fire an epoch rotation. Caller must hold `commit_lock`.
pub fn try_fire_rotation(
    storage: &GitStorage,
    current_branch: &str,
    threshold: u64,
    archive_dir: &Path,
    author: (&str, &str),
    created_at: &str,
) -> Result<RotationOutcome, RotationError> {
    // Zero-loss guard (review I3): the Lost path resets hard onto origin, so
    // fire may only proceed from a clean local == origin state. Any backlog
    // (messages committed between push-success and our lock acquisition)
    // defers rotation to the next push.
    if storage.has_unpushed_commits()? {
        return Ok(RotationOutcome::NotReady);
    }

    let n = storage.count_commits_on_branch(current_branch)?;
    if n < threshold {
        return Ok(RotationOutcome::NotReady);
    }

    // Best-effort fetch so the origin checks below see fresh state. If it
    // fails (offline) the atomic push will arbitrate anyway.
    let _ = storage.fetch();

    // Invariant 3: read epoch state from origin, not the working tree.
    let origin_ref = format!("origin/{current_branch}");
    let origin_epoch = epoch_at_ref(storage, &origin_ref)?;
    if matches!(&origin_epoch, Some(f) if f.status == EpochStatus::Redirected) {
        // Someone already rotated this branch — we are a follower, not a firer.
        return Ok(RotationOutcome::Lost);
    }
    let current_epoch = origin_epoch.as_ref().map(|f| f.epoch).unwrap_or(1);
    let new_epoch = current_epoch + 1;
    let new_branch = format!("main-epoch-{new_epoch}");

    let sealed_commit_sha = storage.rev_parse(current_branch)?.trim().to_string();
    let sealed_short = &sealed_commit_sha[..7];
    let archive_tag = format!("archive/epoch-{current_epoch}/{sealed_short}");

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

    let orphan_commit_sha = storage.create_orphan_commit(
        &new_branch,
        EPOCH_FILE,
        &active_yaml,
        &format!("snapshot: open epoch {new_epoch} from {current_branch}@{sealed_short}"),
        author,
    )?;
    storage.write_redirect_commit(
        EPOCH_FILE,
        &redirect_yaml,
        &format!(
            "seal: redirect epoch {current_epoch} -> {new_branch}@{}",
            &orphan_commit_sha[..7]
        ),
        author,
    )?;

    match storage.atomic_push_two_refs(current_branch, &new_branch) {
        Ok(()) => {
            storage.checkout_branch(&new_branch)?;
            // Best-effort archive: tag + push + bundle. Failure warns, never
            // blocks — the rotation itself is already durable on origin.
            if let Err(e) = storage.tag_archive(&archive_tag, &sealed_commit_sha) {
                tracing::warn!("rotation: tag_archive failed (non-fatal): {e}");
            } else if let Err(e) = storage.push_tag(&archive_tag) {
                tracing::warn!("rotation: push_tag failed (non-fatal): {e}");
            }
            let bundle_path = archive_dir.join(format!("epoch-{current_epoch}.bundle"));
            if let Err(e) = storage.bundle_to_path(&bundle_path, &archive_tag) {
                tracing::warn!("rotation: bundle failed (non-fatal): {e}");
            }
            Ok(RotationOutcome::Won {
                sealed_branch: current_branch.to_string(),
                new_branch,
                new_epoch,
                sealed_commit_sha,
                orphan_commit_sha,
            })
        }
        Err(GitError::PushConflict) => {
            // Lost the race (to another firer OR to a plain message push —
            // design scenarios 1 and 2; we don't need to know which).
            cleanup_failed_fire(storage, current_branch, &new_branch)?;
            Ok(RotationOutcome::Lost)
        }
        Err(e) => {
            // Auth / rate-limit / network (review C1 follow-through): nobody
            // won; restore the clean state and let a later push retry.
            cleanup_failed_fire(storage, current_branch, &new_branch)?;
            Err(RotationError::Git(e))
        }
    }
}
```

`lib.rs` 加 `pub mod rotate;`。

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -5
```

Expected: 3 测试 PASS（NotReady / solo Won / Lost 清理）。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync && git commit -m "feat(sync): rotate module — try_fire_rotation with origin-authoritative checks and loss cleanup"
```

---

### Task 5: rotate.rs — resolve_active_branch / migrate_unpushed / follow_redirect / check_push_fence

**Files:**
- Modify: `crates/gitim-sync/src/rotate.rs`
- Modify: `crates/gitim-sync/tests/rotate_test.rs`

- [ ] **Step 1: Write failing tests**

```rust
use gitim_sync::rotate::{check_push_fence, follow_redirect};

#[test]
fn follow_noop_when_origin_active() {
    let (_bare, clone) = setup_bare_and_clone(2);
    let storage = GitStorage::new(clone.path());
    let acted = follow_redirect(&storage, "main").unwrap();
    assert!(!acted);
    assert_eq!(head_branch(&clone), "main");
}

#[test]
fn follow_switches_and_migrates_unpushed() {
    // A fire 成功后，B 本地有一条未推送消息 commit → follow 应把它搬到新分支。
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);

    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(&storage_a, "main", 3, arch.path(),
        ("a", "a@g"), "2026-06-10T00:00:00Z").unwrap();
    assert!(matches!(o, RotationOutcome::Won { .. }));

    // B 不知情，本地写一条"消息"（未推送）。
    commit_file(&clone_b, "general.thread", "[L1][@b][2026-06-10T00:00:01Z] hello");

    let storage_b = GitStorage::new(clone_b.path());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-2");
    // 未推送消息迁移成功：内容在新分支工作树上。
    assert!(clone_b.path().join("general.thread").exists());
    // epoch.yaml 是 active 版（来自新分支），不是 redirect 版。
    let yaml = std::fs::read_to_string(clone_b.path().join("gitim.epoch.yaml")).unwrap();
    assert!(yaml.contains("status: active"));
}

#[test]
fn follow_resolves_across_two_epochs() {
    // 连续两次 rotation（A fire epoch2，再 fire epoch3），沉睡的 B 一次 follow 到 epoch3。
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t1").unwrap(),
        RotationOutcome::Won { .. }
    ));
    // epoch2 上灌 3 个 commit 再 fire 一次。
    for i in 0..3 { commit_file(&clone_a, &format!("e2-{i}.txt"), "x"); }
    git(&clone_a, &["push", "origin", "main-epoch-2"]);
    assert!(matches!(
        try_fire_rotation(&storage_a, "main-epoch-2", 3, arch.path(), ("a", "a@g"), "t2").unwrap(),
        RotationOutcome::Won { .. }
    ));

    let storage_b = GitStorage::new(clone_b.path());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-3");
}

#[test]
fn fence_blocks_push_when_head_redirected() {
    // B pull 到 R 之后（HEAD tree 的 epoch.yaml = redirected）→ fence 必须报 true。
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    git(&clone_b, &["fetch", "origin"]);
    git(&clone_b, &["reset", "--hard", "origin/main"]); // 模拟 pull 到 R
    // R 之上又写了一条消息（场景 4）。
    commit_file(&clone_b, "late.thread", "[L1][@b][t] late msg");

    let storage_b = GitStorage::new(clone_b.path());
    assert!(check_push_fence(&storage_b).unwrap(), "HEAD carries redirected epoch.yaml");

    // 干净 active 分支上 fence 必须放行。
    assert!(!check_push_fence(&storage_a).unwrap());
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-sync --test rotate_test follow 2>&1 | tail -3
```

Expected: compile error。

- [ ] **Step 3: Implement**

Append to `rotate.rs`:

```rust
/// Walk the redirect chain from `start_branch` (reading each
/// `origin/<b>:gitim.epoch.yaml`) until an active/absent epoch file.
/// Returns the final branch. Errors after MAX_FOLLOW_HOPS (cycle guard).
pub fn resolve_active_branch(
    storage: &GitStorage,
    start_branch: &str,
) -> Result<String, RotationError> {
    let mut branch = start_branch.to_string();
    for _ in 0..MAX_FOLLOW_HOPS {
        let origin_ref = format!("origin/{branch}");
        match epoch_at_ref(storage, &origin_ref)? {
            Some(f) if f.status == EpochStatus::Redirected => {
                let target = f
                    .redirect
                    .as_ref()
                    .ok_or_else(|| {
                        RotationError::Epoch("redirected but no redirect block".into())
                    })?
                    .target_branch
                    .clone();
                branch = target;
            }
            _ => return Ok(branch),
        }
    }
    Err(RotationError::Epoch(format!(
        "redirect chain exceeded {MAX_FOLLOW_HOPS} hops from {start_branch}"
    )))
}

/// True iff HEAD's committed tree carries a redirected epoch.yaml — i.e. a
/// redirect commit R is in the local chain. O(1) and complete: R writes the
/// redirected yaml and no message commit ever touches that file (invariant 1).
pub fn check_push_fence(storage: &GitStorage) -> Result<bool, RotationError> {
    Ok(matches!(
        epoch_at_ref(storage, "HEAD")?,
        Some(f) if f.status == EpochStatus::Redirected
    ))
}

/// Transplant unpushed commits of `from_branch` onto `target_branch`
/// (design migrate, scenario 3/4). The redirect boundary is wherever
/// `origin/<from_branch>` points (R or older); everything after it locally
/// is user content that must survive. On rebase conflict the caller falls
/// back to sync_loop's capture-and-replay path.
pub fn migrate_unpushed(
    storage: &GitStorage,
    from_branch: &str,
    target_branch: &str,
) -> Result<(), RotationError> {
    let origin_from = format!("origin/{from_branch}");
    let origin_target = format!("origin/{target_branch}");
    storage.rebase_onto(&origin_target, &origin_from)?;
    Ok(())
}

/// Follow a redirect published on origin: resolve the final active branch
/// (multi-hop), carry any unpushed local commits over, and switch the
/// checkout. Caller must hold `commit_lock`. Returns true if a switch
/// happened. Decisions read origin state only (invariant 3) — a no-op when
/// origin says active, regardless of local residue.
pub fn follow_redirect(
    storage: &GitStorage,
    current_branch: &str,
) -> Result<bool, RotationError> {
    storage.fetch()?;
    let target = resolve_active_branch(storage, current_branch)?;
    if target == current_branch {
        return Ok(false);
    }

    let has_unpushed = storage.has_unpushed_commits().unwrap_or(false);

    // Make the target branch exist locally, tracking origin.
    storage.create_or_repoint_branch(&target)?;

    if has_unpushed {
        // HEAD is on current_branch; transplant <origin/current>..HEAD onto
        // origin/target. After this HEAD is detached on the migrated chain;
        // repoint the target branch at it, then checkout.
        migrate_unpushed(storage, current_branch, &target)?;
        storage.repoint_branch_to_head(&target)?;
    }
    storage.checkout_branch(&target)?;

    // Old local branch may still carry pre-redirect commits; align it to
    // origin so nothing ever resurrects content onto the sealed branch.
    let _ = storage.reset_to_origin_without_checkout(current_branch);
    // reset_branch_to_origin would checkout; we must stay on `target`. See helper below.

    Ok(true)
}
```

需要的三个小 helper（`git.rs`）：

```rust
    /// `git branch -f <branch> origin/<branch>` — create or re-point a local
    /// branch at its origin counterpart without checkout.
    pub fn create_or_repoint_branch(&self, branch: &str) -> Result<(), GitError> {
        let origin_ref = format!("origin/{branch}");
        run_git(&["branch", "-f", branch, &origin_ref], &self.root).map(|_| ())
    }

    /// `git branch -f <branch> HEAD` — after a rebase leaves HEAD detached on
    /// the migrated chain, stamp the branch there.
    pub fn repoint_branch_to_head(&self, branch: &str) -> Result<(), GitError> {
        run_git(&["branch", "-f", branch, "HEAD"], &self.root).map(|_| ())
    }

    /// `git update-ref refs/heads/<branch> origin/<branch>` — align a NON-checked-out
    /// branch to origin（不动工作树；与 reset_branch_to_origin 的区别是后者 checkout）。
    pub fn reset_to_origin_without_checkout(&self, branch: &str) -> Result<(), GitError> {
        let origin_sha = self.rev_parse(&format!("origin/{branch}"))?;
        let refname = format!("refs/heads/{branch}");
        run_git(&["update-ref", &refname, origin_sha.trim()], &self.root).map(|_| ())
    }
```

- [ ] **Step 4: Run to verify pass**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -5
```

Expected: 7 测试 PASS。

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync && git commit -m "feat(sync): follow_redirect (multi-hop, migrating) + push fence"
```

---

### Task 6: Race 测试套件补全

**Files:**
- Modify: `crates/gitim-sync/tests/rotate_test.rs`

- [ ] **Step 1: Write tests（设计矩阵 1 / 3 / 7 的剩余场景）**

```rust
#[test]
fn race_two_daemons_only_one_wins_other_follows() {
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let storage_b = GitStorage::new(clone_b.path());
    let (arch_a, arch_b) = (tempfile::TempDir::new().unwrap(), tempfile::TempDir::new().unwrap());

    let oa = try_fire_rotation(&storage_a, "main", 3, arch_a.path(), ("a", "a@g"), "t").unwrap();
    let ob = try_fire_rotation(&storage_b, "main", 3, arch_b.path(), ("b", "b@g"), "t").unwrap();
    assert!(matches!(oa, RotationOutcome::Won { .. }));
    assert!(matches!(ob, RotationOutcome::Lost));

    // Loser follow 后双方收敛同一分支；loser 本地无残留。
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    for cl in [&clone_a, &clone_b] {
        assert_eq!(head_branch(cl), "main-epoch-2");
    }
    let out = Command::new("git").args(["log", "--oneline", "main", "-1"])
        .current_dir(clone_b.path()).output().unwrap();
    let local_main_tip = String::from_utf8_lossy(&out.stdout);
    assert!(local_main_tip.contains("seal: redirect"),
        "loser's local main must equal origin (winner's R), got: {local_main_tip}");
}

#[test]
fn normal_push_loses_to_fire_message_migrates() {
    // 场景 3 的端到端版：B 写消息、fire 已发生 → B 的消息经 migrate 出现在新分支
    // 且 sealed branch 上 R 之后没有任何 commit（不变量 1）。
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));

    commit_file(&clone_b, "ch.thread", "[L1][@b][t] msg born on sealed branch");
    let storage_b = GitStorage::new(clone_b.path());
    // B 的 push 必然 reject（origin/main 已含 R）——sync_loop 此时会走 fence + follow。
    assert!(storage_b.push().is_err());
    let acted = follow_redirect(&storage_b, "main").unwrap();
    assert!(acted);
    assert_eq!(head_branch(&clone_b), "main-epoch-2");
    assert!(clone_b.path().join("ch.thread").exists(), "message survived migration");

    // 推上去后验证不变量 1：origin/main tip 仍是 R。
    git(&clone_b, &["push", "origin", "main-epoch-2"]);
    git(&clone_b, &["fetch", "origin"]);
    let tip_msg = Command::new("git").args(["log", "-1", "--format=%s", "origin/main"])
        .current_dir(clone_b.path()).output().unwrap();
    assert!(String::from_utf8_lossy(&tip_msg.stdout).starts_with("seal: redirect"),
        "sealed branch tip must remain the redirect commit");
}

#[test]
fn fire_with_unpushed_backlog_returns_not_ready() {
    // Review I3：push 成功到 fire 拿锁之间 handler 又写了消息 → fire 必须让路，
    // 否则 Lost 清理的 reset --hard 会摧毁这条消息。
    let (_bare, clone) = setup_bare_and_clone(5);
    commit_file(&clone, "inflight.thread", "[L1][@x][t] committed but not pushed");

    let storage = GitStorage::new(clone.path());
    let arch = tempfile::TempDir::new().unwrap();
    let o = try_fire_rotation(&storage, "main", 3, arch.path(),
        ("d", "d@g"), "2026-06-10T00:00:00Z").unwrap();
    assert!(matches!(o, RotationOutcome::NotReady), "got {o:?}");
    assert!(clone.path().join("inflight.thread").exists(), "backlog must survive");
    assert_eq!(head_branch(&clone), "main");
}

#[test]
fn cleanup_refuses_when_foreign_commits_ahead() {
    // Review I3：ahead-of-origin 区间含非自产 commit → cleanup 拒绝 reset。
    let (_bare, clone) = setup_bare_and_clone(3);
    commit_file(&clone, "user-msg.thread", "[L1][@x][t] precious");
    let storage = GitStorage::new(clone.path());

    gitim_sync::rotate::cleanup_failed_fire(&storage, "main", "main-epoch-2").unwrap();
    assert!(clone.path().join("user-msg.thread").exists(),
        "foreign commit must not be reset away");
}

#[test]
fn boot_cleanup_resets_partial_fire_residue() {
    // 场景 7：atomic push 前 crash 的残留 = 本地 main 上有 R'、本地有废 orphan 分支，
    // 而 origin 干净。cleanup_failed_fire 之后一切归位。
    let (_bare, clone) = setup_bare_and_clone(3);
    let storage = GitStorage::new(clone.path());

    // 手工制造残留：写 redirect commit + 开 orphan 分支（模拟 fire 中途死亡）。
    // Subject 必须以 "seal: redirect" 开头——cleanup 的自产验证按此前缀放行。
    let redirect = gitim_core::epoch::EpochFile::new_redirect(
        1, "main".into(), 2, "main-epoch-2".into(),
        "deadbeef".into(), "deadbeef".into(), "t".into(), None);
    let yaml = serde_yaml::to_string(&redirect).unwrap();
    storage.write_redirect_commit("gitim.epoch.yaml", &yaml,
        "seal: redirect epoch 1 -> main-epoch-2 (partial fire)", ("d", "d@g")).unwrap();
    storage.create_orphan_commit("main-epoch-2", "gitim.epoch.yaml", "status: active\n",
        "snapshot: partial", ("d", "d@g")).unwrap();

    gitim_sync::rotate::cleanup_failed_fire(&storage, "main", "main-epoch-2").unwrap();

    assert_eq!(head_branch(&clone), "main");
    assert!(!clone.path().join("gitim.epoch.yaml").exists());
    assert_eq!(storage.rev_parse("main").unwrap(), storage.rev_parse("origin/main").unwrap());
}
```

注意：`create_orphan_commit` 的真实签名以 cherry-pick 进来的实现为准——写测试前先读 `git.rs` 确认参数顺序，必要时调整调用。

- [ ] **Step 2: Run**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -5
```

Expected: 全 PASS。任何 FAIL 都是 Task 4/5 实现与矩阵的偏差——修实现，不改测试语义。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-sync/tests/rotate_test.rs
git commit -m "test(sync): race matrix coverage — fire-vs-fire, fire-vs-write both directions, boot cleanup"
```

---

### Task 7: sync_loop 接入 fence + migrate 路径

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs`（`sync_with_push`，约 542-700 行）
- Test: `crates/gitim-sync/tests/rotate_test.rs`（fence 集成场景）

**插入点逻辑**（三处，全部锁外检查、锁内处置）：

1. **direct push 前**（`sync_with_push` 顶部、`repo.push()` 之前）：`check_push_fence` → true → 拿 `commit_lock` 调 `follow_redirect` → 返回 `SyncOutcome::Normal`（本 cycle 结束，消息下个 cycle 从新分支推）。
2. **fetch 成功后、rebase 前**（fence (i)）：读 `origin/<branch>:gitim.epoch.yaml`（`epoch_at_ref` 经 rotate 模块暴露，或 sync_loop 直接用 `show_file_at_ref` + serde 判定）→ redirected → 拿锁 follow（含 migrate）→ 返回。**不**把本地消息 rebase 到 R 上。
3. **rebase 后 push 前**（fence (ii) 兜底）：`check_push_fence` → true → 拿锁 follow → 返回。

- [ ] **Step 1: Write failing integration test**

Append to `rotate_test.rs`：

```rust
use gitim_sync::sync_loop::run_sync_cycle_for_test; // 若无此入口，则直接测 sync_with_push 的可见效果：
// 本测试通过「构造 B 有未推送消息 + origin 已 rotate」后调用一轮 sync，断言消息上了新分支。

#[test]
fn sync_cycle_routes_message_to_new_epoch_after_rotation() {
    let (bare, clone_a) = setup_bare_and_clone(3);
    let clone_b = clone_from(&bare);
    let storage_a = GitStorage::new(clone_a.path());
    let arch = tempfile::TempDir::new().unwrap();
    assert!(matches!(
        try_fire_rotation(&storage_a, "main", 3, arch.path(), ("a", "a@g"), "t").unwrap(),
        RotationOutcome::Won { .. }
    ));
    commit_file(&clone_b, "late.thread", "[L1][@b][t] written before B knows");

    // 跑一轮真实 sync cycle（被 fence 拦截 → follow → 下一轮 push 到新分支）。
    let storage_b = GitStorage::new(clone_b.path());
    let lock = std::sync::Mutex::new(());
    gitim_sync::sync_loop::sync_once_for_test(&storage_b, &lock); // 见 Step 3 暴露的测试入口
    gitim_sync::sync_loop::sync_once_for_test(&storage_b, &lock);

    git(&clone_b, &["fetch", "origin"]);
    // 消息在新分支远端可见。
    let out = Command::new("git").args(["show", "origin/main-epoch-2:late.thread"])
        .current_dir(clone_b.path()).output().unwrap();
    assert!(out.status.success(), "message must land on origin/main-epoch-2");
    // sealed branch tip 仍是 R。
    let tip = Command::new("git").args(["log", "-1", "--format=%s", "origin/main"])
        .current_dir(clone_b.path()).output().unwrap();
    assert!(String::from_utf8_lossy(&tip.stdout).starts_with("seal: redirect"));
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-sync --test rotate_test sync_cycle_routes 2>&1 | tail -3
```

- [ ] **Step 3: Implement**

`sync_with_push` 开头（`for attempt` 循环之前）加：

```rust
    // Epoch fence, checkpoint (ii)-direct: if HEAD already carries a
    // redirected epoch.yaml (R pulled in a previous cycle, scenario 4),
    // publishing anything from here would violate invariant 1. Follow first;
    // local commits migrate inside follow_redirect.
    if epoch_fence_and_follow(repo, commit_lock) {
        return SyncOutcome::Normal;
    }
```

fetch `Ok(()) => {}` 分支之后（`enforce_max_divergence` 之前）加 fence (i)：

```rust
        // Epoch fence, checkpoint (i): origin tip became a redirect while we
        // were composing (scenario 3). Do NOT rebase local messages onto R —
        // follow (which migrates them) instead.
        if let Ok(branch) = repo.current_branch() {
            let origin_ref = format!("origin/{branch}");
            if matches!(
                crate::rotate::epoch_status_at_ref(repo, &origin_ref),
                Ok(Some(gitim_core::epoch::EpochStatus::Redirected))
            ) {
                if epoch_fence_and_follow(repo, commit_lock) {
                    return SyncOutcome::Normal;
                }
            }
        }
```

rebase 成功后、`push_after_rebase` 之前加 fence (ii)：

```rust
                // Epoch fence, checkpoint (ii): the rebase may have replayed
                // local messages on top of R. Never publish that.
                if epoch_fence_and_follow(repo, commit_lock) {
                    return SyncOutcome::Normal;
                }
```

文件底部加共享 helper：

```rust
/// Shared fence handler: when HEAD (or caller-detected origin state) says
/// redirected, take commit_lock and follow. Returns true when this cycle
/// must NOT push (a follow happened, or fencing state is unresolved).
///
/// Fail-closed (review): a CORRUPT epoch.yaml (epoch_status_at_ref Err) is
/// treated as fenced — we refuse to push rather than risk publishing onto a
/// sealed branch we can't read.
fn epoch_fence_and_follow(repo: &GitStorage, commit_lock: &Mutex<()>) -> bool {
    // Err => fenced (fail-closed), None/Active => not fenced.
    let head_fenced = !matches!(
        crate::rotate::epoch_status_at_ref(repo, "HEAD"),
        Ok(None) | Ok(Some(gitim_core::epoch::EpochStatus::Active))
    );
    let origin_fenced = || -> bool {
        repo.current_branch().ok().is_some_and(|b| {
            !matches!(
                crate::rotate::epoch_status_at_ref(repo, &format!("origin/{b}")),
                Ok(None) | Ok(Some(gitim_core::epoch::EpochStatus::Active))
            )
        })
    };
    if !head_fenced && !origin_fenced() {
        return false;
    }
    let _guard = commit_lock
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let branch = match repo.current_branch() {
        Ok(b) => b,
        Err(e) => {
            warn!("epoch fence: current_branch failed: {e}");
            return true; // unresolved — keep the fence closed this cycle
        }
    };

    // Self-heal (review I5): HEAD redirected but ORIGIN active means a Lost
    // cleanup failed earlier and left R' stranded locally. Retry the cleanup
    // now, BEFORE any pull-rebase can mix message commits above R' (which
    // would force human intervention). Pure-seal verification inside
    // cleanup_failed_fire keeps this safe.
    if head_fenced && !origin_fenced() {
        let orphan = crate::rotate::epoch_file_at_ref(repo, "HEAD")
            .ok()
            .flatten()
            .and_then(|f| f.redirect.map(|r| r.target_branch))
            .unwrap_or_default();
        if let Err(e) = crate::rotate::cleanup_failed_fire(repo, &branch, &orphan) {
            warn!("epoch fence: residue cleanup failed: {e}");
        }
        return true; // don't push this cycle either way; next cycle re-evaluates
    }

    match crate::rotate::follow_redirect(repo, &branch) {
        Ok(acted) => {
            if acted {
                info!("epoch fence: followed redirect off sealed branch {branch}");
            }
            acted
        }
        Err(e) => {
            warn!("epoch fence: follow_redirect failed: {e}");
            // Stay on the sealed branch but NEVER push from it this cycle.
            true
        }
    }
}
```

`rotate.rs` 把 `epoch_at_ref` 包装暴露为 `pub fn epoch_status_at_ref(storage, ref) -> Result<Option<EpochStatus>, RotationError>`（sync_loop 只需要 status，不需要整个 file）。

测试入口 `sync_once_for_test`：`pub fn sync_once_for_test(repo: &GitStorage, lock: &Mutex<()>)` —— 以 no-op 回调调一轮 `sync_with_push`（按文件内现有测试辅助惯例放 `#[doc(hidden)]` 或 `pub(crate)` + 集成测试通过现有 public API；若现有测试已有等价入口直接复用）。

**migrate 冲突兜底**：`follow_redirect` 内 `migrate_unpushed` 返回 Err 时——`follow_redirect` 把错误上抛，`epoch_fence_and_follow` 返回 true（cycle 结束、不 push）。下一轮 cycle fence 再触发重试；重复冲突时本地消息保持未推送（不丢），现有 `conflict::resolve_content` 路径在 rebase 冲突分支已覆盖 `.thread` 内容级合并（场景 8 的 daemon 集成测试在 Task 8 验证）。

- [ ] **Step 4: Run**

```bash
cargo test -p gitim-sync --test rotate_test 2>&1 | tail -5
cargo test -p gitim-sync 2>&1 | tail -5   # sync_loop 存量测试不破
```

- [ ] **Step 5: Commit**

```bash
cargo fmt -p gitim-sync
git add crates/gitim-sync && git commit -m "feat(sync): epoch push-fence in sync_with_push — never publish onto a sealed branch"
```

---

### Task 8: daemon 接线 — threshold / throttle / on_pushed / on_synced / boot

**Files:**
- Modify: `crates/gitim-daemon/src/state.rs`（on_pushed ~328 行、on_synced ~376-457 行）
- Modify: `crates/gitim-daemon/src/main.rs`（boot，~156 行 `refresh_epoch_status` 附近）
- Create: `crates/gitim-daemon/tests/epoch_rotation.rs`

- [ ] **Step 1: Write failing integration test**

```rust
//! Daemon-level rotation: threshold override fires rotation through the
//! same path the on_pushed hook uses; migrate conflict falls back cleanly.

use std::process::Command;
use tempfile::TempDir;

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(Command::new("git").args(args).current_dir(dir).status().unwrap().success());
}

#[tokio::test]
async fn daemon_auto_rotates_when_threshold_crossed() {
    // 不依赖 env var（多线程 test race）——attempt_rotation_for_test 接受显式 threshold。
    let bare = TempDir::new().unwrap();
    let clone = TempDir::new().unwrap();
    git(bare.path(), &["init", "--bare", "-b", "main"]);
    git(clone.path(), &["clone", bare.path().to_str().unwrap(), "."]);
    git(clone.path(), &["config", "user.email", "d@d"]);
    git(clone.path(), &["config", "user.name", "d"]);
    for i in 0..3 {
        std::fs::write(clone.path().join(format!("f{i}")), "x").unwrap();
        git(clone.path(), &["add", "."]);
        git(clone.path(), &["commit", "-m", &format!("c{i}")]);
    }
    git(clone.path(), &["push", "-u", "origin", "main"]);

    // AppState 构造按现有 epoch_gate.rs / 其他 daemon 集成测试的既有模式。
    let state = gitim_daemon::test_support::app_state_for(clone.path()); // 以现有测试基建为准
    let fired = tokio::task::spawn_blocking(move || state.attempt_rotation_for_test(3))
        .await.unwrap().unwrap();
    assert!(fired);

    let head = Command::new("git").args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(clone.path()).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&head.stdout).trim(), "main-epoch-2");
}
```

执行注意：先读 `crates/gitim-daemon/tests/` 现有集成测试（如 `epoch_gate.rs` 若存在、或任一构造 AppState 的测试）确定 AppState 测试构造的真实写法，替换 `test_support::app_state_for` 占位调用——**plan 此处刻意不硬编码构造代码，以仓库现状为准**。

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p gitim-daemon --test epoch_rotation 2>&1 | tail -3
```

- [ ] **Step 3: Implement state.rs wiring**

常量 + throttle 字段：

```rust
/// Default production threshold: 1,000,000 commits per epoch.
pub const ROTATION_THRESHOLD_DEFAULT: u64 = 1_000_000;
/// Min interval between rotation checks — avoids running `rev-list --count`
/// on a million-commit repo after every push. The threshold is soft; a few
/// hundred commits of overshoot are irrelevant by design.
const ROTATION_CHECK_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

fn rotation_threshold() -> u64 {
    std::env::var("GITIM_ROTATION_THRESHOLD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(ROTATION_THRESHOLD_DEFAULT)
}
```

`AppState` 加字段 `last_rotation_check: std::sync::Mutex<Option<std::time::Instant>>`（`new()` 初始化 `Mutex::new(None)`）。

`impl AppState`：

```rust
    /// Test entry: same path as the on_pushed hook, explicit threshold.
    pub fn attempt_rotation_for_test(&self, threshold: u64) -> Result<bool, String> {
        self.try_rotate_inner(threshold).map_err(|e| e.to_string())
    }

    fn rotation_check_due(&self) -> bool {
        let mut last = self
            .last_rotation_check
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = std::time::Instant::now();
        match *last {
            Some(t) if now.duration_since(t) < ROTATION_CHECK_INTERVAL => false,
            _ => {
                *last = Some(now);
                true
            }
        }
    }

    /// Acquire commit_lock and run the fire/follow state machine once.
    fn try_rotate_inner(
        &self,
        threshold: u64,
    ) -> Result<bool, gitim_sync::rotate::RotationError> {
        let _guard = self
            .commit_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let storage = gitim_sync::git::GitStorage::new(&self.repo_root);
        let branch = storage.current_branch()?;
        let archive_dir = self.repo_root.join(".gitim").join("archive");
        let (name, email) = self.rotation_author();
        let created_at = chrono::Utc::now().to_rfc3339();

        let outcome = gitim_sync::rotate::try_fire_rotation(
            &storage, &branch, threshold, &archive_dir,
            (name.as_str(), email.as_str()), &created_at,
        )?;
        let fired = match outcome {
            gitim_sync::rotate::RotationOutcome::Won { .. } => true,
            gitim_sync::rotate::RotationOutcome::Lost => {
                gitim_sync::rotate::follow_redirect(&storage, &branch)?;
                false
            }
            gitim_sync::rotate::RotationOutcome::NotReady => return Ok(false),
        };
        if let Err(e) = self.refresh_epoch_status() {
            tracing::warn!("rotation: epoch status refresh failed: {e}");
        }
        Ok(fired)
    }

    /// (name, email) stamped on rotation commits — mirrors the rebase author
    /// resolution so every clone keeps single-author attribution.
    fn rotation_author(&self) -> (String, String) { /* 复用/参照现有 rebase_author 逻辑实现 */ }
```

`rotation_author` 以 state.rs 现有的 rebase author 推导（sync_loop 的 `rebase_author` 参数来源）为准复用——读现有代码后实现，不重复造。

`on_pushed` 闭包末尾（drain + broadcast 之后）：

```rust
                    // Rotation check: this push may have tipped the epoch over
                    // threshold. Throttled (60s) because counting a 1M-commit
                    // branch isn't free; blocking pool because it's all git I/O.
                    if push_state.rotation_check_due() {
                        let st = push_state.clone();
                        tokio::task::spawn_blocking(move || {
                            if let Err(e) = st.try_rotate_inner(rotation_threshold()) {
                                tracing::warn!("rotation attempt failed: {e}");
                            }
                        });
                    }
```

`on_synced` 闭包，`refresh_epoch_status` 之后：

```rust
                    // Sync may have pulled a redirect from origin. Follow now —
                    // the push fence already guarantees we can't publish onto
                    // the sealed branch meanwhile.
                    if synced_state.is_redirected() {
                        let st = synced_state.clone();
                        tokio::task::spawn_blocking(move || {
                            let _guard = st.commit_lock.lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let storage = gitim_sync::git::GitStorage::new(&st.repo_root);
                            let branch = match storage.current_branch() {
                                Ok(b) => b,
                                Err(e) => { tracing::warn!("on_synced follow: {e}"); return; }
                            };
                            match gitim_sync::rotate::follow_redirect(&storage, &branch) {
                                Ok(true) => { let _ = st.refresh_epoch_status(); }
                                Ok(false) => {}
                                Err(e) => tracing::warn!("on_synced follow failed: {e}"),
                            }
                        });
                    }
```

`main.rs` boot（`refresh_epoch_status` 调用后）：

```rust
    // Crash residue cleanup + catch-up follow (design scenario 7): a partial
    // fire leaves a local redirect commit that origin never saw; a completed
    // fire we crashed before checking out leaves us on the sealed branch.
    {
        let st = app_state.clone();
        let storage = gitim_sync::git::GitStorage::new(&st.repo_root);
        if let Ok(branch) = storage.current_branch() {
            let origin_redirected = matches!(
                gitim_sync::rotate::epoch_status_at_ref(&storage, &format!("origin/{branch}")),
                Ok(Some(gitim_core::epoch::EpochStatus::Redirected))
            );
            let head_redirected =
                matches!(gitim_sync::rotate::check_push_fence(&storage), Ok(true));
            if head_redirected && !origin_redirected {
                let orphan_guess = format!(
                    "main-epoch-{}",
                    gitim_sync::rotate::epoch_status_at_ref(&storage, "HEAD")
                        .ok().flatten().map(|_| 0).unwrap_or(0) // 占位——实现时读 HEAD epoch file 的 redirect.target_branch
                );
                // 实现要点：cleanup 的 orphan 分支名从 HEAD 的 epoch.yaml redirect.target_branch 读，
                // 而不是猜编号。rotate.rs 暴露 `epoch_at_ref` 的 pub 包装即可。
                if let Err(e) =
                    gitim_sync::rotate::cleanup_failed_fire(&storage, &branch, &orphan_guess)
                {
                    tracing::warn!("boot: partial-fire cleanup failed: {e}");
                }
            } else if origin_redirected {
                let _guard = st.commit_lock.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if let Err(e) = gitim_sync::rotate::follow_redirect(&storage, &branch) {
                    tracing::warn!("boot: follow_redirect failed: {e}");
                }
                let _ = st.refresh_epoch_status();
            }
        }
    }
```

（boot 块里的 `orphan_guess` 占位在实现时必须替换为"从 HEAD epoch file 读 `redirect.target_branch`"——rotate.rs 加 `pub fn epoch_file_at_ref(...) -> Result<Option<EpochFile>, _>` 包装。）

- [ ] **Step 4: 加 migrate 冲突回退集成测试（场景 8）**

Append to `epoch_rotation.rs`：

```rust
#[tokio::test]
async fn migrate_conflict_falls_back_to_renumber() {
    // B 的未推送消息与新 epoch 上已推送的消息在同一 thread 文件同一行号 →
    // rebase --onto 冲突 → cycle 不推不丢 → 下一轮 conflict::resolve_content
    // 路径以 renumber 收敛。断言：B 的消息最终出现在 origin/main-epoch-2，
    // 行号被重排，sealed branch tip 仍是 R。
    // 构造：A fire 后在 epoch-2 写 general.thread [L1]；B 在旧 main 写 general.thread [L1]。
    // 跑两轮 daemon sync cycle 后验证。
    // （实现按 daemon 现有 sync 测试基建驱动 run_sync_cycle；具体构造以 Step 1 同款 helper 为准。）
    todo!("flesh out with the same helpers as daemon_auto_rotates_when_threshold_crossed");
}
```

执行注意：这个测试**必须真实落地**（删 `todo!`，写完整构造）——它是场景 8 的唯一验收。写不出来 = sync_loop 的 migrate 回退路径有问题，回 Task 7 修。

- [ ] **Step 5: Run**

```bash
cargo test -p gitim-daemon --test epoch_rotation 2>&1 | tail -5
cargo test -p gitim-daemon 2>&1 | tail -5   # daemon 存量测试不破
```

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/gitim-daemon && git commit -m "feat(daemon): wire epoch auto-rotation — on_pushed fire, on_synced follow, boot cleanup"
```

---

### Task 9: runtime health 字段 + daemon-web 只读拦截

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs`（health handler）
- Modify: `products/gitim/frontend/src/daemon-web/state.ts`
- Modify: `products/gitim/frontend/src/daemon-web/sync.ts`
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`
- Test: `products/gitim/frontend/src/daemon-web/handlers.test.ts`

- [ ] **Step 1: runtime health**

`/runtime/health` 响应结构加两个字段（找到现有 health struct，加）：

```rust
    /// Number of epochs this workspace has rotated through (1 = never rotated).
    pub epoch_count: u32,
    /// Commit count on the human clone's current branch (rotation progress).
    pub total_commit_count: Option<u64>,
```

取值：human clone 的 `GitStorage` → `count_commits_on_branch(current_branch)`（失败 → None）；epoch_count 从 `gitim.epoch.yaml` 读（无文件 = 1）。现有 health test 加断言字段存在。

- [ ] **Step 2: daemon-web 只读拦截（TDD）**

`handlers.test.ts` 加测试（按文件内现有测试惯例）：

```typescript
describe('epoch redirect interception', () => {
  it('send returns epoch_redirected error when state is redirected', async () => {
    // 按现有测试 setup 初始化，然后注入 redirected 状态：
    setState({ epochRedirected: true });
    const res = await send('general', 'hello');
    expect(res.ok).toBe(false);
    expect((res as any).error_code).toBe('epoch_redirected');
  });
});
```

实现三处：

`state.ts` — `DaemonWebState` 加 `epochRedirected: boolean`（initState 默认 false）。

`sync.ts` — sync 循环每轮 pull 后检测 repo 根 `gitim.epoch.yaml`（用现有 storage 读文件 API），`status: redirected` → `setState({ epochRedirected: true })` 并停止后续 push（read-only：sync 继续 pull，跳过 push）。

`handlers.ts` — 加 guard，写入类 handler（`send` / `joinChannel` / `archiveChannel` / `unarchiveChannel` / `archiveDm` / `unarchiveDm` / `publishBoard` / `setBoard` / `setBoardSectionValue` 及 card/flow 写入口——以文件内全部产生 commit 的 handler 为准）开头调用：

```typescript
function assertNotRedirected(): ApiResponse | null {
  if (getState().epochRedirected) {
    return {
      ok: false,
      error_code: 'epoch_redirected',
      error: 'This workspace has rotated to a new epoch branch. Reload the page to reconnect.',
    } as ApiResponse;
  }
  return null;
}
// 各写入 handler 开头：
// const blocked = assertNotRedirected(); if (blocked) return blocked;
```

前端 UI：调用层拿到 `error_code === 'epoch_redirected'` 时 toast/banner 提示刷新——找到前端统一错误处理位置（`products/gitim/frontend/src` 下消费 ApiResponse 错误的地方）加一个分支即可，v1 不做自动重连。

- [ ] **Step 3: Run**

```bash
cargo test -p gitim-runtime 2>&1 | tail -3
cd products/gitim/frontend && npx vitest run src/daemon-web/handlers.test.ts 2>&1 | tail -5
```

- [ ] **Step 4: Commit**

```bash
git add crates/gitim-runtime products/gitim/frontend
git commit -m "feat: epoch observability in /runtime/health + daemon-web read-only interception"
```

---

### Task 10: CLAUDE.md orientation + 收尾验证

**Files:**
- Modify: `CLAUDE.md`（Current Orientation 段）

- [ ] **Step 1: CLAUDE.md 更新**

`Current Orientation` 的 **Where we are** 末尾加一段（风格对齐现有条目）：

> **Epoch auto-rotation (Snapshot Pack Phase B v2)** 已落地：daemon 在 on_pushed 后（60s throttle）数当前分支 commit 数，过阈值（默认 1M，`GITIM_ROTATION_THRESHOLD` 覆盖）自动 fire——orphan snapshot 开 `main-epoch-{N+1}` + 老分支 seal redirect commit，`git push --atomic` 双 ref 仲裁单 winner，loser 清理本地残留转 follow。三条协议不变量：sealed tip 永远是 R / atomic push 是唯一仲裁 / 判定一律以 origin 为准。sync_loop 三处 push-fence（direct push 前 / fetch 后 rebase 前 / rebase 后 push 前）保证消息永不发布到 sealed branch；被拦消息经 `rebase --onto` migrate 到新分支（冲突回退现有 renumber 路径），零丢失。follow 多跳（max 32）一次到位；boot 时清半成品 fire 残留。winner 落本地 bundle（`.gitim/archive/epoch-N.bundle`）+ archive tag。daemon-web v1 只做只读拦截（`epoch_redirected` 错误 + 刷新提示）。`/runtime/health` 暴露 `epoch_count` / `total_commit_count`。竞态矩阵见 `docs/plans/git-history-snapshot-pack/03-phase-b-v2-design.md`。

**Where we're going** 里加：epoch v2 留位（新 clone single-branch 优化、bundle 上传、auto-prune、WebUI epoch 状态）。

- [ ] **Step 2: 收尾验证（scoped，不跑全量）**

```bash
cargo test -p gitim-sync 2>&1 | tail -3
cargo test -p gitim-daemon --test epoch_rotation 2>&1 | tail -3
cargo clippy --workspace --all-targets --no-deps --locked 2>&1 | tail -3
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: orient epoch auto-rotation in CLAUDE.md"
```

---

## Self-Review 记录

- **Spec coverage**：设计矩阵 8 场景 → 测试映射表全覆盖（场景 5 是进程内锁语义，由场景 4 的 fence 测试 + commit_lock 既有不变式覆盖，无独立测试）✓
- **Placeholder**：Task 8 的 `test_support::app_state_for` 与 boot 块 `orphan_guess` 是**有意的执行期决策点**（以仓库现状为准），均已标注替换要求；Task 8 Step 4 的 `todo!` 标注了"必须真实落地"的验收要求 ✓
- **Type consistency**：`epoch_status_at_ref` / `epoch_file_at_ref` / `check_push_fence` / `follow_redirect` / `cleanup_failed_fire` 签名在 Task 5/7/8 间一致 ✓
