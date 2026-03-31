# Meta YAML 迁移 + Sync 冲突解决

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 将 meta 文件从 JSON 切换为 YAML 格式，并扩展 sync_loop 冲突解决以支持 meta 文件的自动合并（members 取并集）。

**Architecture:** meta 文件改为 YAML 格式（`.meta.yaml`），每个 member 占一行，利用 git 的行级合并减少冲突。当冲突仍然发生时，sync_loop 的自定义冲突解决会捕获本地 meta 变更、discard 后与远端版本合并（members 取并集，标量取远端），然后写回并提交。

**Tech Stack:** Rust + serde_yaml 0.9（workspace 已有）

---

## File Structure

| Action | File | Responsibility |
|--------|------|----------------|
| Modify | `crates/gitim-core/src/types/meta.rs` | 无结构变更，仅确认 serde derives 兼容 YAML |
| Modify | `crates/gitim-core/src/validator/mod.rs` | `validate_user_meta` / `validate_channel_meta`: JSON→YAML 解析 |
| Modify | `crates/gitim-core/tests/validator_test.rs` | 测试输入从 JSON→YAML |
| Modify | `crates/gitim-sync/Cargo.toml` | 添加 `serde_yaml` + `serde` 依赖 |
| Modify | `crates/gitim-sync/src/conflict.rs` | 新增 `merge_channel_meta` 函数 |
| Modify | `crates/gitim-sync/src/git.rs` | 新增 `changed_files_unpushed` 方法 |
| Modify | `crates/gitim-sync/src/sync_loop.rs` | 扩展冲突解决：捕获+合并 meta 文件 |
| Modify | `crates/gitim-sync/src/watcher.rs` | `.meta.json` → `.meta.yaml` |
| Modify | `crates/gitim-daemon/src/onboard.rs` | 所有 meta 路径和序列化：JSON→YAML |
| Modify | `crates/gitim-daemon/src/handlers.rs` | 所有 meta 路径和序列化：JSON→YAML |
| Modify | `crates/gitim-daemon/src/main.rs` | 用户扫描 `.meta.json` → `.meta.yaml` |
| Modify | `crates/gitim-sync/tests/conflict_test.rs` | 新增 meta merge 测试 |
| Modify | `crates/gitim-sync/tests/sync_e2e_test.rs` | 新增 meta 冲突解决 e2e 测试 |
| Modify | `crates/gitim-sync/tests/onboard_test.rs` | .meta.json → .meta.yaml（~18 处）|
| Modify | `crates/gitim-sync/tests/git_ops_test.rs` | 新增 changed_files_unpushed 测试 |
| Modify | `crates/gitim-daemon/tests/commit_test.rs` | user meta 路径和格式 |
| Modify | `crates/gitim-daemon/tests/push_test.rs` | user meta 路径和格式 |
| Modify | `crates/gitim-daemon/tests/push_confirm_test.rs` | user meta 路径和格式 |
| Modify | `crates/gitim-index/src/lib.rs` | 测试断言字符串更新 |
| Modify | `tests/e2e_test.sh` | meta 路径和格式 |

---

## Chunk 1: gitim-core — 格式切换

### Task 1: 更新 validator — YAML 解析

**Files:**
- Modify: `crates/gitim-core/src/validator/mod.rs:23-73`
- Modify: `crates/gitim-core/tests/validator_test.rs`

- [ ] **Step 1: 更新 validator_test.rs — 将 meta 测试输入改为 YAML**

```rust
// crates/gitim-core/tests/validator_test.rs
// 只改 test_valid_user_meta, test_user_meta_missing_field, test_user_meta_display_name_too_long,
// test_valid_channel_meta, test_channel_meta_missing_field, test_channel_meta_invalid_created_at,
// test_channel_meta_invalid_created_by

#[test]
fn test_valid_user_meta() {
    let yaml = "display_name: Nexus\nrole: ceo\nintroduction: hello\n";
    assert!(validate_user_meta(yaml).is_ok());
}

#[test]
fn test_user_meta_missing_field() {
    let yaml = "display_name: Nexus\nrole: ceo\n";
    assert!(validate_user_meta(yaml).is_err());
}

#[test]
fn test_user_meta_display_name_too_long() {
    let name = "x".repeat(65);
    let yaml = format!("display_name: {}\nrole: ceo\nintroduction: hi\n", name);
    assert!(validate_user_meta(&yaml).is_err());
}

#[test]
fn test_valid_channel_meta() {
    let yaml = "display_name: General\ncreated_by: nexus\ncreated_at: \"20250316T120000Z\"\nintroduction: hello\n";
    assert!(validate_channel_meta(yaml).is_ok());
}

#[test]
fn test_channel_meta_missing_field() {
    let yaml = "display_name: General\ncreated_by: nexus\n";
    assert!(validate_channel_meta(yaml).is_err());
}

#[test]
fn test_channel_meta_invalid_created_at() {
    let yaml = "display_name: General\ncreated_by: nexus\ncreated_at: not-a-date\nintroduction: hello\n";
    assert!(validate_channel_meta(yaml).is_err());
}

#[test]
fn test_channel_meta_invalid_created_by() {
    let yaml = "display_name: General\ncreated_by: INVALID\ncreated_at: \"20250316T120000Z\"\nintroduction: hello\n";
    assert!(validate_channel_meta(yaml).is_err());
}
```

Note: `validate_channel_name` 和 `validate_config` 测试无需改动（不涉及 meta 格式）。

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-core --test validator_test 2>&1 | tail -20`
Expected: 7 个 meta 测试 FAIL（传入 YAML 但函数仍用 JSON 解析）

- [ ] **Step 3: 更新 validate_user_meta 和 validate_channel_meta**

`crates/gitim-core/src/validator/mod.rs` — 将 `serde_json::from_str` 改为 `serde_yaml::from_str`：

```rust
pub fn validate_user_meta(yaml: &str) -> Result<UserMeta, ValidationError> {
    let meta: UserMeta = serde_yaml::from_str(yaml)?;
    // ... 其余验证逻辑不变
}

pub fn validate_channel_meta(yaml: &str) -> Result<ChannelMeta, ValidationError> {
    let meta: ChannelMeta = serde_yaml::from_str(yaml)?;
    // ... 其余验证逻辑不变
}
```

同时确认 `ValidationError` 已有 `YamlParse` variant（已有：`YamlParse(#[from] serde_yaml::Error)`）。

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-core --test validator_test 2>&1 | tail -20`
Expected: all 13 tests PASS

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-core/src/validator/mod.rs crates/gitim-core/tests/validator_test.rs
git commit -m "refactor(core): switch meta validators from JSON to YAML parsing"
```

---

## Chunk 2: gitim-sync — Meta 冲突解决

### Task 2: 添加 serde_yaml 依赖到 gitim-sync

**Files:**
- Modify: `crates/gitim-sync/Cargo.toml`

- [ ] **Step 1: 添加依赖**

在 `[dependencies]` 中添加：

```toml
serde.workspace = true
serde_yaml.workspace = true
```

在 `[dev-dependencies]` 中添加：

```toml
serde_yaml.workspace = true
```

（integration tests 中需要直接调用 `serde_yaml::from_str` 验证合并结果）

- [ ] **Step 2: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo check -p gitim-sync 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-sync/Cargo.toml
git commit -m "chore(sync): add serde_yaml dependency for meta conflict resolution"
```

### Task 3: 添加 merge_channel_meta 函数

**Files:**
- Modify: `crates/gitim-sync/src/conflict.rs`
- Modify: `crates/gitim-sync/tests/conflict_test.rs`

- [ ] **Step 1: 写测试**

在 `crates/gitim-sync/tests/conflict_test.rs` 末尾添加：

```rust
use gitim_core::types::ChannelMeta;
use gitim_sync::conflict::merge_channel_meta;

#[test]
fn test_merge_channel_meta_union_members() {
    let local = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "god".into()],
    };
    let remote = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["bob".into(), "god".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.members, vec!["alice", "bob", "god"]);
}

#[test]
fn test_merge_channel_meta_scalars_from_remote() {
    let local = ChannelMeta {
        display_name: "Local Name".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "local intro".into(),
        members: vec!["alice".into()],
    };
    let remote = ChannelMeta {
        display_name: "Remote Name".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "remote intro".into(),
        members: vec!["bob".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.display_name, "Remote Name");
    assert_eq!(merged.introduction, "remote intro");
    assert_eq!(merged.members, vec!["alice", "bob"]);
}

#[test]
fn test_merge_channel_meta_dedup_members() {
    let local = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "bob".into(), "god".into()],
    };
    let remote = ChannelMeta {
        display_name: "General".into(),
        created_by: "god".into(),
        created_at: "20260330T120000Z".into(),
        introduction: "默认频道".into(),
        members: vec!["alice".into(), "god".into()],
    };
    let merged = merge_channel_meta(&local, &remote);
    assert_eq!(merged.members, vec!["alice", "bob", "god"]);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test conflict_test 2>&1 | tail -10`
Expected: FAIL — `merge_channel_meta` 不存在

- [ ] **Step 3: 实现 merge_channel_meta**

在 `crates/gitim-sync/src/conflict.rs` 顶部添加 import，在末尾添加函数：

```rust
// 顶部添加 import
use gitim_core::types::ChannelMeta;

// 末尾添加函数
/// Merge two ChannelMeta: members 取并集（排序去重），标量字段取 remote。
pub fn merge_channel_meta(local: &ChannelMeta, remote: &ChannelMeta) -> ChannelMeta {
    let mut members: Vec<String> = remote.members.clone();
    for m in &local.members {
        if !members.contains(m) {
            members.push(m.clone());
        }
    }
    members.sort();

    ChannelMeta {
        display_name: remote.display_name.clone(),
        created_by: remote.created_by.clone(),
        created_at: remote.created_at.clone(),
        introduction: remote.introduction.clone(),
        members,
    }
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test conflict_test 2>&1 | tail -10`
Expected: all tests PASS（含原有 3 个 + 新增 3 个）

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-sync/src/conflict.rs crates/gitim-sync/tests/conflict_test.rs
git commit -m "feat(sync): add merge_channel_meta for meta conflict resolution"
```

### Task 4: 添加 changed_files_unpushed 到 git.rs

**Files:**
- Modify: `crates/gitim-sync/src/git.rs`
- Modify: `crates/gitim-sync/tests/git_ops_test.rs`

- [ ] **Step 1: 写测试**

在 `crates/gitim-sync/tests/git_ops_test.rs` 末尾添加（使用该文件现有的 `setup_repo_pair()` helper，它返回 `(TempDir, TempDir, GitStorage)`）：

```rust
#[test]
fn test_changed_files_unpushed_detects_meta() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    // Create a meta.yaml file locally
    let ch_dir = clone_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(ch_dir.join("general.meta.yaml"), "display_name: General\n").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add meta"]);

    let changed = repo.changed_files_unpushed("*.meta.yaml").unwrap();
    assert_eq!(changed.len(), 1);
    assert!(changed[0].to_str().unwrap().contains("general.meta.yaml"));
}

#[test]
fn test_changed_files_unpushed_empty_when_pushed() {
    let (_bare_dir, clone_dir, repo) = setup_repo_pair();

    let ch_dir = clone_dir.path().join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    std::fs::write(ch_dir.join("general.meta.yaml"), "display_name: General\n").unwrap();
    run_git(clone_dir.path(), &["add", "."]);
    run_git(clone_dir.path(), &["commit", "-m", "add meta"]);
    repo.push().unwrap();

    let changed = repo.changed_files_unpushed("*.meta.yaml").unwrap();
    assert!(changed.is_empty());
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test git_ops_test test_changed_files 2>&1 | tail -10`
Expected: FAIL — `changed_files_unpushed` 方法不存在

- [ ] **Step 3: 实现 changed_files_unpushed**

在 `crates/gitim-sync/src/git.rs` 的 `impl GitStorage` 块中添加：

```rust
/// List files changed between origin/main and HEAD, matching a pattern.
/// Returns relative paths (e.g. "channels/general.meta.yaml").
pub fn changed_files_unpushed(&self, pattern: &str) -> Result<Vec<PathBuf>, GitError> {
    let output = Command::new("git")
        .args(["diff", "--name-only", "origin/main..HEAD", "--", pattern])
        .current_dir(&self.root)
        .output()?;
    if !output.status.success() {
        return Err(GitError::CommandFailed(
            String::from_utf8_lossy(&output.stderr).to_string(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|l| !l.is_empty())
        .map(PathBuf::from)
        .collect())
}
```

- [ ] **Step 4: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test git_ops_test test_changed_files 2>&1 | tail -10`
Expected: 2 tests PASS

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-sync/src/git.rs crates/gitim-sync/tests/git_ops_test.rs
git commit -m "feat(sync): add changed_files_unpushed to GitStorage"
```

### Task 5: 扩展 sync_loop 的 meta 冲突解决

**Files:**
- Modify: `crates/gitim-sync/src/sync_loop.rs:100-219`
- Modify: `crates/gitim-sync/tests/sync_e2e_test.rs`

- [ ] **Step 1: 写 e2e 测试**

在 `crates/gitim-sync/tests/sync_e2e_test.rs` 末尾添加。使用现有的 `setup_two_clones()` helper（返回 `(TempDir, TempDir, TempDir)`），手动执行 sync flow（与 `test_sync_resolves_concurrent_writes` 同模式，因为 `sync_with_push` 是 private 函数）：

```rust
#[test]
fn test_sync_resolves_concurrent_meta_writes() {
    let (_bare_dir, clone_a_dir, clone_b_dir) = setup_two_clones();
    let repo_a = GitStorage::new(clone_a_dir.path());
    let repo_b = GitStorage::new(clone_b_dir.path());

    // Setup: repo_a creates channel meta.yaml and pushes
    let ch_dir_a = clone_a_dir.path().join("channels");
    let initial_meta = "display_name: General\ncreated_by: god\ncreated_at: \"20260330T120000Z\"\nintroduction: test\nmembers:\n- god\n";
    std::fs::write(ch_dir_a.join("general.meta.yaml"), initial_meta).unwrap();
    repo_a.add_and_commit(&["channels/general.meta.yaml"], "init channel").unwrap();
    repo_a.push().unwrap();

    // repo_b pulls the initial state
    repo_b.pull_rebase().unwrap();

    // repo_a adds alice to members and pushes
    let meta_a = "display_name: General\ncreated_by: god\ncreated_at: \"20260330T120000Z\"\nintroduction: test\nmembers:\n- alice\n- god\n";
    std::fs::write(ch_dir_a.join("general.meta.yaml"), meta_a).unwrap();
    repo_a.add_and_commit(&["channels/general.meta.yaml"], "add alice").unwrap();
    repo_a.push().unwrap();

    // repo_b adds bob to members (from old base) — will conflict on push
    let ch_dir_b = clone_b_dir.path().join("channels");
    let meta_b = "display_name: General\ncreated_by: god\ncreated_at: \"20260330T120000Z\"\nintroduction: test\nmembers:\n- bob\n- god\n";
    std::fs::write(ch_dir_b.join("general.meta.yaml"), meta_b).unwrap();
    repo_b.add_and_commit(&["channels/general.meta.yaml"], "add bob").unwrap();

    // Manual sync flow (mirrors sync_loop logic):
    // 1. Push fails with conflict
    assert!(repo_b.push().is_err());

    // 2. Fetch + capture local meta changes
    repo_b.fetch().unwrap();
    let changed_meta = repo_b.changed_files_unpushed("*.meta.yaml").unwrap();
    assert_eq!(changed_meta.len(), 1);
    let local_meta_content = std::fs::read_to_string(
        clone_b_dir.path().join(&changed_meta[0])
    ).unwrap();
    let local_meta: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(&local_meta_content).unwrap();

    // 3. Discard unpushed
    repo_b.discard_unpushed().unwrap();

    // 4. Read remote version, merge
    let remote_content = std::fs::read_to_string(
        ch_dir_b.join("general.meta.yaml")
    ).unwrap();
    let remote_meta: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(&remote_content).unwrap();
    let merged = gitim_sync::conflict::merge_channel_meta(&local_meta, &remote_meta);

    // 5. Write merged, commit, push
    std::fs::write(
        ch_dir_b.join("general.meta.yaml"),
        serde_yaml::to_string(&merged).unwrap(),
    ).unwrap();
    repo_b.add_and_commit(&["channels/general.meta.yaml"], "meta: merge").unwrap();
    repo_b.push().unwrap();

    // Verify: merged meta has alice + bob + god
    let result_content = std::fs::read_to_string(ch_dir_b.join("general.meta.yaml")).unwrap();
    let result_meta: gitim_core::types::ChannelMeta =
        serde_yaml::from_str(&result_content).unwrap();
    assert_eq!(result_meta.members, vec!["alice", "bob", "god"]);
}
```

- [ ] **Step 2: 运行测试确认失败**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test sync_e2e_test test_sync_resolves_concurrent_meta 2>&1 | tail -20`
Expected: FAIL — meta 冲突时 discard 丢弃了变更

- [ ] **Step 3: 修改 sync_loop.rs**

`crates/gitim-sync/src/sync_loop.rs` — 在 `sync_with_push` 中扩展冲突解决逻辑。

关键修改（在 `for attempt` 循环中，fetch 之后，rebase 之前）：

```rust
// === 现有代码 ===
// Capture local additions BEFORE attempting rebase
let local_additions = match repo.diff_unpushed("*.thread") {
    Ok(v) => v,
    Err(e) => {
        warn!("sync: failed to diff unpushed additions: {}", e);
        return;
    }
};

// === 新增：捕获本地 meta 变更 ===
let changed_meta_files = repo.changed_files_unpushed("*.meta.yaml").unwrap_or_default();
let local_metas: HashMap<PathBuf, String> = changed_meta_files
    .iter()
    .filter_map(|p| {
        let abs = repo.root().join(p);
        std::fs::read_to_string(&abs).ok().map(|c| (p.clone(), c))
    })
    .collect();
```

修改 rebase 失败分支（替换原有的 `Err(_) => { ... }` 分支内容）：

```rust
Err(_) => {
    // Rebase failed — use conflict resolution
    if local_additions.is_empty() && local_metas.is_empty() {
        let _ = repo.discard_unpushed();
        warn!("sync: non-thread non-meta rebase conflict, aborted");
        return;
    }

    if let Err(e) = repo.discard_unpushed() {
        warn!("sync: discard_unpushed failed: {}", e);
        return;
    }

    let mut modified_paths: Vec<String> = Vec::new();
    // 保存 thread mappings 给后面 commit msg 用（避免重复调用 resolve_content）
    let mut thread_mappings: Vec<conflict::RenumberMapping> = Vec::new();

    // Resolve thread conflicts (existing logic)
    if !local_additions.is_empty() {
        match conflict::resolve_content(&local_additions, repo.root()) {
            Ok((resolved_files, mappings)) => {
                for resolved in &resolved_files {
                    let abs_path = repo.root().join(&resolved.path);
                    if let Err(e) = std::fs::write(&abs_path, &resolved.content) {
                        warn!("sync: failed to write resolved file: {}", e);
                        return;
                    }
                    modified_paths.push(resolved.path.to_str().unwrap_or("").to_string());
                }
                for m in &mappings {
                    on_renumbered(m.file.clone(), m.old_line, m.new_line);
                }
                thread_mappings = mappings;
            }
            Err(e) => {
                warn!("sync: conflict resolution failed: {}", e);
                return;
            }
        }
    }

    // Resolve meta conflicts (NEW)
    for (rel_path, local_content) in &local_metas {
        let abs = repo.root().join(rel_path);
        let rel_str = rel_path.to_string_lossy();

        if rel_str.starts_with("channels/") {
            // ChannelMeta: merge members (union + sort)
            let remote_content = std::fs::read_to_string(&abs).ok();
            let merged_yaml = match (
                serde_yaml::from_str::<gitim_core::types::ChannelMeta>(local_content),
                remote_content.as_deref().and_then(|c| serde_yaml::from_str::<gitim_core::types::ChannelMeta>(c).ok()),
            ) {
                (Ok(local_meta), Some(remote_meta)) => {
                    let merged = conflict::merge_channel_meta(&local_meta, &remote_meta);
                    serde_yaml::to_string(&merged).unwrap_or_else(|_| local_content.clone())
                }
                (Ok(_local_meta), None) => {
                    // Remote doesn't have this file — use local as-is
                    local_content.clone()
                }
                _ => {
                    warn!("sync: failed to parse meta for merge: {}", rel_str);
                    continue;
                }
            };
            if let Some(parent) = abs.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&abs, &merged_yaml) {
                warn!("sync: failed to write merged meta: {}", e);
                continue;
            }
        } else {
            // UserMeta or other: write local content back as-is
            if let Some(parent) = abs.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = std::fs::write(&abs, local_content) {
                warn!("sync: failed to write meta: {}", e);
                continue;
            }
        }
        modified_paths.push(rel_path.to_str().unwrap_or("").to_string());
    }

    // Commit all resolved content（使用前面保存的 thread_mappings）
    if !modified_paths.is_empty() {
        let path_refs: Vec<&str> = modified_paths.iter().map(|s| s.as_str()).collect();
        let commit_msg = if !thread_mappings.is_empty() {
            build_rebase_commit_msg(&thread_mappings, &local_additions)
        } else {
            "meta: sync after rebase".to_string()
        };
        if let Err(e) = repo.add_and_commit(&path_refs, &commit_msg) {
            warn!("sync: commit after conflict resolution failed: {}", e);
            return;
        }
    }

    // ... push + retry 逻辑保持不变（现有的 match repo.push() + continue）
}
```

- [ ] **Step 4: 添加必要的 imports**

在 `sync_loop.rs` 顶部添加：

```rust
use std::collections::HashMap;
use gitim_core::types::ChannelMeta;
```

注意：`ChannelMeta` import 用于类型标注（虽然代码中通过 `serde_yaml::from_str::<gitim_core::types::ChannelMeta>` 全限定路径调用，如果改为 `serde_yaml::from_str::<ChannelMeta>` 则需要此 import）。`serde_yaml` 通过 crate 依赖自动可用，无需 `use serde_yaml`。已有的 `use crate::conflict` 涵盖 `merge_channel_meta`。

- [ ] **Step 5: 运行测试确认通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test -p gitim-sync --test sync_e2e_test 2>&1 | tail -20`
Expected: all tests PASS（含原有 3 个 + 新增 1 个）

- [ ] **Step 6: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-sync/src/sync_loop.rs crates/gitim-sync/tests/sync_e2e_test.rs
git commit -m "feat(sync): extend conflict resolution to merge meta files on rebase failure"
```

### Task 6: 更新 watcher.rs

**Files:**
- Modify: `crates/gitim-sync/src/watcher.rs:45-46`

- [ ] **Step 1: 修改文件匹配模式**

```rust
// 将第 45-46 行从:
} else if filename.ends_with(".meta.json") {
    let name = filename.trim_end_matches(".meta.json").to_string();
// 改为:
} else if filename.ends_with(".meta.yaml") {
    let name = filename.trim_end_matches(".meta.yaml").to_string();
```

- [ ] **Step 2: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo check -p gitim-sync 2>&1 | tail -5`
Expected: no errors

- [ ] **Step 3: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-sync/src/watcher.rs
git commit -m "refactor(sync): update watcher to detect .meta.yaml instead of .meta.json"
```

---

## Chunk 3: gitim-daemon — 所有 meta 路径和序列化

### Task 7: 更新 onboard.rs

**Files:**
- Modify: `crates/gitim-daemon/src/onboard.rs`

所有改动均为机械替换。以下列出每处修改：

- [ ] **Step 1: ensure_repo — 频道 meta 路径和序列化**

```
第 185 行: general.meta.json → general.meta.yaml
第 188-194 行: serde_json::json!({...}) → 构造 ChannelMeta struct + serde_yaml::to_string
第 195 行: serde_json::to_string_pretty → serde_yaml::to_string
第 198 行: channels/general.meta.json → channels/general.meta.yaml
```

具体代码：

```rust
let meta_path = channels_dir.join("general.meta.yaml");
if !meta_path.exists() {
    let now = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    let meta = ChannelMeta {
        display_name: "General".to_string(),
        created_by: handler.to_string(),
        created_at: now.clone(),
        introduction: "默认频道".to_string(),
        members: vec![handler.to_string()],
    };
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    std::fs::write(&meta_path, &meta_str)
        .map_err(|e| Response::error(format!("failed to write general.meta.yaml: {}", e)))?;
    changed_paths.push("channels/general.meta.yaml".to_string());
    // ... thread 部分不变
}
```

- [ ] **Step 2: register_user — 用户 meta 路径和序列化**

```
第 252 行: {}.meta.json → {}.meta.yaml
第 257-261 行: serde_json::json!({...}) → 构造 UserMeta struct + serde_yaml::to_string
第 266 行: users/{}.meta.json → users/{}.meta.yaml
```

具体代码：

```rust
let meta_path = users_dir.join(format!("{}.meta.yaml", handler));
if meta_path.exists() {
    return Ok(false);
}

let meta = UserMeta {
    display_name: display_name.to_string(),
    role: "member".to_string(),
    introduction: "GitIM user".to_string(),
};
let meta_str = serde_yaml::to_string(&meta).unwrap();
std::fs::write(&meta_path, &meta_str)
    .map_err(|e| Response::error(format!("failed to write user meta: {}", e)))?;

let rel_path = format!("users/{}.meta.yaml", handler);
```

- [ ] **Step 3: auto_join_general — meta 路径和序列化**

```
第 310 行: channels/general.meta.json → channels/general.meta.yaml
第 317 行: serde_json::from_str → serde_yaml::from_str
第 346 行: serde_json::to_string_pretty → serde_yaml::to_string
第 352 行: channels/general.meta.json → channels/general.meta.yaml
```

具体代码：

```rust
fn auto_join_general(state: &SharedState, handler: &str) -> Result<(), Response> {
    let meta_path = state.repo_root.join("channels/general.meta.yaml");
    if !meta_path.exists() {
        return Ok(());
    }

    let meta_content = std::fs::read_to_string(&meta_path)
        .map_err(|e| Response::error(format!("read meta: {}", e)))?;
    let mut meta: ChannelMeta = serde_yaml::from_str(&meta_content)
        .map_err(|e| Response::error(format!("parse meta: {}", e)))?;

    if meta.members.contains(&handler.to_string()) {
        return Ok(());
    }

    // ... thread event 写入逻辑不变 ...

    meta.members.push(handler.to_string());
    meta.members.sort();
    let meta_str = serde_yaml::to_string(&meta).unwrap();
    std::fs::write(&meta_path, &meta_str)
        .map_err(|e| Response::error(format!("write meta: {}", e)))?;

    let _ = state.git_storage.add_and_commit_as(
        &["channels/general.thread", "channels/general.meta.yaml"],
        &format!("event: @{} join general", handler),
        Some(handler),
    );

    Ok(())
}
```

- [ ] **Step 4: 添加 imports**

在 onboard.rs 顶部确认有：

```rust
use gitim_core::types::{ChannelMeta, UserMeta};
```

如果现有 import 路径不同，按照文件中已有的模式调整。

- [ ] **Step 5: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo check -p gitim-daemon 2>&1 | tail -10`
Expected: 可能还有 handlers.rs 和 main.rs 的编译错误（下个 task 修），但 onboard.rs 部分应无 YAML 相关错误

- [ ] **Step 6: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-daemon/src/onboard.rs
git commit -m "refactor(daemon): migrate onboard.rs meta from JSON to YAML"
```

### Task 8: 更新 handlers.rs

**Files:**
- Modify: `crates/gitim-daemon/src/handlers.rs`

以下修改均为机械替换。

- [ ] **Step 1: handle_list_channels — 路径和解析**

```
第 467 行: .meta.json → .meta.yaml
第 468 行: .meta.json → .meta.yaml
第 471 行: serde_json::from_str::<ChannelMeta> → serde_yaml::from_str::<ChannelMeta>
```

- [ ] **Step 2: handle_poll — 路径和解析**

```
第 634 行: .meta.json → .meta.yaml（strip_suffix）
第 641 行: {}.meta.json → {}.meta.yaml（format!）
第 643 行: serde_json::from_str::<ChannelMeta> → serde_yaml::from_str::<ChannelMeta>
第 669 行: .meta.json → .meta.yaml（strip_suffix）
```

- [ ] **Step 3: handle_register_user — 路径和序列化**

```
第 414 行: format!("{}.meta.json", handler) → format!("{}.meta.yaml", handler)
第 425-430 行: serde_json::json!({...}) + serde_json::to_string_pretty → UserMeta struct + serde_yaml::to_string
第 447 行: format!("users/{}.meta.json", handler) → format!("users/{}.meta.yaml", handler)
```

同时在 handlers.rs 顶部的 import 行添加 `UserMeta`：

```rust
use gitim_core::types::{ChannelMeta, Handler, Link, LinkKind, ThreadEntry, UserMeta};
```

- [ ] **Step 4: write_channel_event — 路径和序列化**

```
第 796 行: {}.meta.json → {}.meta.yaml（format!）
第 799 行: serde_json::from_str → serde_yaml::from_str
第 889 行: serde_json::to_string_pretty → serde_yaml::to_string
第 896 行: channels/{}.meta.json → channels/{}.meta.yaml
```

- [ ] **Step 5: 确认编译通过**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo check -p gitim-daemon 2>&1 | tail -10`
Expected: 可能还有 main.rs 和测试的编译错误

- [ ] **Step 6: 更新 handlers.rs 中引用 "meta.json" 的注释**

搜索并更新以下注释（不影响功能但保持一致性）：
- `// 扫描 channels/*.meta.json` → `// 扫描 channels/*.meta.yaml`
- `// Read channel meta.json` → `// Read channel meta.yaml`
- `// Update meta.json members` → `// Update meta.yaml members`

- [ ] **Step 7: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-daemon/src/handlers.rs
git commit -m "refactor(daemon): migrate handlers.rs meta from JSON to YAML"
```

### Task 9: 更新 main.rs + 所有测试文件

**Files:**
- Modify: `crates/gitim-daemon/src/main.rs:49-51`
- Modify: `crates/gitim-daemon/src/handlers.rs:938-1037`（inline tests）
- Modify: `crates/gitim-daemon/src/onboard.rs`（inline tests）
- Modify: `crates/gitim-daemon/tests/commit_test.rs`
- Modify: `crates/gitim-daemon/tests/push_test.rs`
- Modify: `crates/gitim-daemon/tests/push_confirm_test.rs`
- Modify: `crates/gitim-sync/tests/onboard_test.rs`

- [ ] **Step 1: main.rs — 用户扫描模式**

```rust
// 第 50-51 行改为:
if name.ends_with(".meta.yaml") {
    let handler = name.trim_end_matches(".meta.yaml").to_string();
```

- [ ] **Step 2: handlers.rs tests — 更新 helper 函数**

`register_test_user` helper（约 994-1016 行）：

```rust
async fn register_test_user(state: &SharedState, handler: &str) {
    let users_dir = state.repo_root.join("users");
    std::fs::create_dir_all(&users_dir).unwrap();
    let meta = serde_yaml::to_string(&gitim_core::types::UserMeta {
        display_name: handler.to_string(),
        role: "member".to_string(),
        introduction: "test user".to_string(),
    }).unwrap();
    std::fs::write(
        users_dir.join(format!("{}.meta.yaml", handler)),
        &meta,
    )
    .unwrap();
    let rel = format!("users/{}.meta.yaml", handler);
    let _ = state
        .git_storage
        .add_and_commit(&[&rel], &format!("user: register @{}", handler));
    let mut users = state.users.write().await;
    if !users.contains(&handler.to_string()) {
        users.push(handler.to_string());
        users.sort();
    }
}
```

`create_test_channel` helper（约 1019-1037 行）：

```rust
fn create_test_channel(state: &SharedState, name: &str, created_by: &str) {
    let ch_dir = state.repo_root.join("channels");
    std::fs::create_dir_all(&ch_dir).unwrap();
    let meta = ChannelMeta {
        display_name: name.to_string(),
        created_by: created_by.to_string(),
        created_at: "20260323T000000Z".to_string(),
        introduction: "test channel".to_string(),
        members: Vec::new(),
    };
    std::fs::write(
        ch_dir.join(format!("{}.meta.yaml", name)),
        serde_yaml::to_string(&meta).unwrap(),
    )
    .unwrap();
    std::fs::write(ch_dir.join(format!("{}.thread", name)), "").unwrap();
    let meta_rel = format!("channels/{}.meta.yaml", name);
    let thread_rel = format!("channels/{}.thread", name);
    let _ = state.git_storage.add_and_commit(
        // ... 其余不变
    );
}
```

- [ ] **Step 3: 更新 onboard.rs inline 测试中的 meta 格式**

搜索 onboard.rs 中 `#[cfg(test)]` 后的所有测试，将所有 `.meta.json` 引用和 `serde_json` 用于 meta 的地方改为 `.meta.yaml` 和 `serde_yaml`。关键检查点：
- `general.meta.json` 路径 → `general.meta.yaml`
- `serde_json::from_str::<ChannelMeta>` → `serde_yaml::from_str::<ChannelMeta>`
- `serde_json::Value` 用于 meta 解析 → `serde_yaml::Value` + `serde_yaml::from_str`
- `{handler}.meta.json` → `{handler}.meta.yaml`

- [ ] **Step 4: 更新 3 个 standalone daemon 测试文件**

这些文件创建 user meta 做测试 setup，需要把路径和内容从 JSON 改为 YAML：

**`crates/gitim-daemon/tests/commit_test.rs`:**
- `root.join("users/alice.meta.json")` → `root.join("users/alice.meta.yaml")`
- JSON content → YAML content（`serde_yaml::to_string(&UserMeta { ... })`）

**`crates/gitim-daemon/tests/push_test.rs`:**
- `root.join("users/alice.meta.json")` → `.meta.yaml`
- `state.repo_root.join("users/bob.meta.json")` → `.meta.yaml`
- 所有 JSON meta content → YAML

**`crates/gitim-daemon/tests/push_confirm_test.rs`:**
- `root.join("users/alice.meta.json")` → `.meta.yaml`（多处）
- `rival.path().join("users/bob.meta.json")` → `.meta.yaml`
- 所有 JSON meta content → YAML

- [ ] **Step 6: 更新 gitim-sync/tests/onboard_test.rs**

该文件有 ~18 处 `.meta.json` 引用。全部替换：
- `general.meta.json` → `general.meta.yaml`
- `alice.meta.json` / `bob.meta.json` → `.meta.yaml`
- `serde_json` 用于 meta 解析的地方 → `serde_yaml`

- [ ] **Step 7: 运行全部测试**

Run: `cd /Users/lewisliu/ateam/GitIM && cargo test 2>&1 | tail -30`
Expected: all tests PASS

如果有失败，检查是否遗漏了某处 `.meta.json` 引用：

```bash
cd /Users/lewisliu/ateam/GitIM && grep -rn "meta\.json" crates/ --include="*.rs"
```

Expected: 零结果（除了 `config` 相关的注释或字符串字面量）

- [ ] **Step 5: Commit**

```bash
cd /Users/lewisliu/ateam/GitIM/.worktrees/fix-werewolf-bugs
git add crates/gitim-daemon/
git commit -m "refactor(daemon): migrate main.rs and all tests from JSON to YAML meta"
```

---

## Chunk 4: 验证和收尾

### Task 10: 全量验证 + 外围文件

- [ ] **Step 1: 更新 e2e shell 测试**

`tests/e2e_test.sh` 中有 3 处 `.meta.json` 引用需要改为 `.meta.yaml`，meta 内容从 JSON 改为 YAML 格式。

- [ ] **Step 2: 更新 gitim-index 测试断言**

`crates/gitim-index/src/lib.rs` 约第 785 行：

```rust
// 改为:
assert_eq!(parse_diff_path("users/alice.meta.yaml"), None);
```

（功能不受影响，但保持一致性）

- [ ] **Step 3: 确认无残留 `.meta.json` 引用**

```bash
cd /Users/lewisliu/ateam/GitIM && grep -rn "\.meta\.json" crates/ tests/ --include="*.rs" --include="*.sh" | grep -v "//.*meta\.json"
```

Expected: 零结果（排除注释中的引用）

- [ ] **Step 4: 运行完整测试套件**

```bash
cd /Users/lewisliu/ateam/GitIM && cargo test 2>&1 | tail -30
```

Expected: all ~95+ tests PASS

- [ ] **Step 5: 编译 release binary**

```bash
cd /Users/lewisliu/ateam/GitIM && cargo build --release -p gitim-daemon 2>&1 | tail -5
```

Expected: 编译成功

- [ ] **Step 6: 安装新 daemon**

```bash
cp /Users/lewisliu/ateam/GitIM/target/release/gitim-daemon ~/.cargo/bin/
```

- [ ] **Step 7: 最终 commit（如有遗漏修复）**

只在前面有遗漏需要修复时才需要。
