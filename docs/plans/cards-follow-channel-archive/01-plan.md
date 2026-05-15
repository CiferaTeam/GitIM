# Cards Follow Channel Archive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `archive_channel` 把 `channels/<ch>/cards/` 子目录一起 mv 到 archive,unarchive 反向只复活"via channel"的卡片;后端启动幂等 reconcile 现有孤儿目录。Rust daemon 和 frontend daemon-web 双端落地。

**Architecture:** `CardMeta` 加 `archived_via: channel | manual | null` 字段做来源 marker。`archive_channel` / `unarchive_channel` 改为多文件单 commit;`archive_card` / `unarchive_card` set/unset 字段。启动跑 `reconcile_orphan_cards`(Rust daemon `AppState::new` 后 + frontend worker boot 后),扫 `channels/<archived-ch>/cards/` 孤儿目录 → 一次性 mv + 字段写入 + commit;无孤儿 no-op。

**Tech Stack:** Rust(`gitim-core` / `gitim-daemon`), TypeScript(`products/gitim/frontend/src/daemon-web/`), serde-yaml, isomorphic-git, vitest, cargo test

**Spec:** `docs/plans/cards-follow-channel-archive/00-design.md`

---

## Phase 0: Baseline 全量测试

### Task 0.1: Rust workspace baseline

- [ ] **Step 1:** 切到 worktree 根 `/Users/lewisliu/ateam/GitIM/.claude/worktrees/sweet-chatelet-7eff6a/`,跑 `cargo test --workspace`。
- [ ] **Step 2:** 记录:总测试数、是否全 green、是否有 `#[ignore]` 测试(本计划不需要 unignore)。
- [ ] **Step 3:** 若有 baseline 红测试,在本计划顶部加一段 "Baseline as of YYYY-MM-DD" 临时段,列出已知红测试名 + 是否本计划相关。任务结束删除这段。

### Task 0.2: Frontend baseline

- [ ] **Step 1:** `cd products/gitim/frontend && pnpm install`(若 node_modules 缺,worktree 第一次跑必要)
- [ ] **Step 2:** `pnpm exec tsc -b` —— 应 0 错误。
- [ ] **Step 3:** `pnpm test` —— 应全 green(参考前置 PR commit 48423b0 baseline:31 test files / 261 tests)。
- [ ] **Step 4:** 若有 red,记录在 Task 0.1 同一临时段。

---

## Phase 1: gitim-core CardMeta 字段扩展

### Task 1.1: 加 `ArchivedVia` enum + `CardMeta.archived_via` 字段

**Files:**
- Modify: `crates/gitim-core/src/types/card.rs` (line 39-78)
- Test: 同文件内联 `#[cfg(test)] mod tests`(若已有则追加,无则新建)

- [ ] **Step 1: 写失败测试**

在 `crates/gitim-core/src/types/card.rs` 末尾追加(若已有 `#[cfg(test)] mod tests` 则塞入):

```rust
#[cfg(test)]
mod archived_via_tests {
    use super::*;

    #[test]
    fn archived_via_serializes_lowercase() {
        let yaml = serde_yaml::to_string(&ArchivedVia::Channel).unwrap();
        assert_eq!(yaml.trim(), "channel");
        let yaml = serde_yaml::to_string(&ArchivedVia::Manual).unwrap();
        assert_eq!(yaml.trim(), "manual");
    }

    #[test]
    fn card_meta_omits_archived_via_when_none() {
        let meta = CardMeta {
            title: "t".into(),
            channel: "c".into(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: None,
            created_by: "u".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            archived_via: None,
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(!yaml.contains("archived_via"),
            "expected omitted field, got:\n{yaml}");
    }

    #[test]
    fn card_meta_writes_archived_via_when_some() {
        let meta = CardMeta {
            title: "t".into(),
            channel: "c".into(),
            status: CardStatus::Todo,
            labels: vec![],
            assignee: None,
            created_by: "u".into(),
            created_at: "2026-01-01T00:00:00Z".into(),
            updated_at: "2026-01-01T00:00:00Z".into(),
            archived_via: Some(ArchivedVia::Channel),
        };
        let yaml = serde_yaml::to_string(&meta).unwrap();
        assert!(yaml.contains("archived_via: channel"),
            "expected field present, got:\n{yaml}");
    }

    #[test]
    fn card_meta_reads_legacy_yaml_without_field() {
        let yaml = "title: t\nchannel: c\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: u\ncreated_at: '2026-01-01T00:00:00Z'\nupdated_at: '2026-01-01T00:00:00Z'\n";
        let meta: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.archived_via, None);
    }

    #[test]
    fn card_meta_reads_archived_via_channel() {
        let yaml = "title: t\nchannel: c\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: u\ncreated_at: '2026-01-01T00:00:00Z'\nupdated_at: '2026-01-01T00:00:00Z'\narchived_via: channel\n";
        let meta: CardMeta = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(meta.archived_via, Some(ArchivedVia::Channel));
    }
}
```

- [ ] **Step 2: 跑测试,确认失败**

```
cargo test -p gitim-core archived_via_tests
```

Expected: 编译失败,`error[E0422]: cannot find struct ArchivedVia` 和 `no field archived_via on type CardMeta`。

- [ ] **Step 3: 实现 ArchivedVia + CardMeta 字段**

`crates/gitim-core/src/types/card.rs` 在现有 `CardStatus` enum(line 39-45)下方插入:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchivedVia {
    Channel,
    Manual,
}
```

修改 `CardMeta`(line 66-78),在 `updated_at` 后加字段:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CardMeta {
    pub title: String,
    pub channel: String,
    pub status: CardStatus,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub assignee: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archived_via: Option<ArchivedVia>,
}
```

- [ ] **Step 4: 跑测试,确认通过**

```
cargo test -p gitim-core archived_via_tests
```

Expected: 5 passed。

- [ ] **Step 5: 跑全 gitim-core 测试,确认没破其他**

```
cargo test -p gitim-core
```

Expected: 全 green。

- [ ] **Step 6: 修编译错误**

`cargo build --workspace` 可能因为 `CardMeta` 的构造点(无字段初始化)报 missing field 错。修每一处:加 `archived_via: None`。常见 call site:

- `crates/gitim-daemon/src/card_handlers.rs` `handle_create_card`
- 任何 `CardMeta {...}` 直接构造(可用 `rg "CardMeta\s*\{"` 找)

不修 `..Default::default()` 模式(本 struct 无 Default impl)。

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-core/src/types/card.rs crates/gitim-daemon/src/card_handlers.rs
git commit -m "feat(core): add ArchivedVia and CardMeta.archived_via field

Optional field with serde skip_serializing_if so legacy yaml without
the field reads as None and active card yaml stays clean.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 2: Rust daemon archive_card / unarchive_card 字段写入

### Task 2.1: `archive_card` 把 `archived_via: manual` 写入 yaml

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs:249-376`(`handle_archive_card`)
- Test: `crates/gitim-daemon/tests/cards.rs` 或类似已有 archive_card 集成测试文件

- [ ] **Step 1: 定位现有 archive_card 测试**

```bash
rg -l "archive_card|handle_archive_card" crates/gitim-daemon/tests/
```

读现有测试结构,沿用既有 fixture pattern(`TestRepo::new`、`create_channel` 等 helpers)。

- [ ] **Step 2: 写失败测试**

在合适测试文件加(filename 取决于现有结构,这里用 `tests/cards.rs` 假设):

```rust
#[tokio::test]
async fn archive_card_sets_archived_via_manual_in_yaml() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let card = repo.create_card("general", "alice", "task").await;

    repo.archive_card("general", &card.card_id, "alice").await.unwrap();

    // 文件应在 archive/channels/general/cards/<id>/card.meta.yaml
    let path = repo.path().join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        card.card_id
    ));
    let yaml = std::fs::read_to_string(&path).expect("archived card meta exists");
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(
        meta.archived_via,
        Some(gitim_core::types::ArchivedVia::Manual)
    );
}
```

- [ ] **Step 3: 跑失败**

```
cargo test -p gitim-daemon archive_card_sets_archived_via_manual_in_yaml
```

Expected: 失败 — `archived_via` 仍为 `None`(代码没写)。

- [ ] **Step 4: 实现 — 在 mv 前改 yaml**

`crates/gitim-daemon/src/card_handlers.rs`,在 `handle_archive_card`(line 249)的 step 5/6 之间(line 304-311),把 read meta 那段改成 read + mutate + write:

```rust
    // 5. Read card.meta.yaml
    let meta_path = state
        .repo_root
        .join(&located.rel_path)
        .join("card.meta.yaml");
    let mut meta: gitim_core::types::CardMeta = match std::fs::read_to_string(&meta_path) {
        Ok(c) => match serde_yaml::from_str(&c) {
            Ok(m) => m,
            Err(e) => return Response::error(format!("failed to parse card meta: {}", e)),
        },
        Err(_) => {
            return Response::error(format!(
                "card '{}' not found in channel '{}'",
                card_id, channel
            ))
        }
    };

    // 6. Permission check: only creator or assignee can archive
    let is_creator = meta.created_by == author;
    let is_assignee = meta.assignee.as_deref() == Some(author.as_str());
    if !is_creator && !is_assignee {
        return Response::error("only creator or assignee can archive");
    }

    // 6b. Stamp archived_via: manual, write back BEFORE git mv so the mv carries the new yaml.
    meta.archived_via = Some(gitim_core::types::ArchivedVia::Manual);
    let new_yaml = match serde_yaml::to_string(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("failed to serialize card meta: {}", e)),
    };
    if let Err(e) = std::fs::write(&meta_path, new_yaml) {
        return Response::error(format!("failed to write card meta: {}", e));
    }
```

- [ ] **Step 5: 跑测试,确认通过**

```
cargo test -p gitim-daemon archive_card_sets_archived_via_manual_in_yaml
```

Expected: pass。

- [ ] **Step 6: 跑 daemon 全测,确认没破其他**

```
cargo test -p gitim-daemon
```

Expected: 全 green(注意 daemon 集成测试稍慢,~1-2 min)。

- [ ] **Step 7: 也加 "archive_card commit rollback also undoes yaml write" 测试**

archive_card 在 commit 失败时 rollback git mv(line 341-348)。新逻辑加了 yaml mutate 步骤,失败时也要 rollback yaml。先加 test 暴露这个缺口:

```rust
#[tokio::test]
async fn archive_card_rolls_back_yaml_when_commit_fails() {
    // 用一个 mocked git_storage 让 commit 失败,验证 archive 后 yaml archived_via 仍为 None
    // (具体 fixture 依赖现有测试基础设施 — 若无 mock 能力,改为 `#[ignore]` 并加注释说明手动验证)
    // ...
}
```

若 daemon 测试 infra 没法 mock commit 失败(常见情形),把这个测试标 `#[ignore]` 加注释:"manual: simulate by chmod 0444 on .git/ then archive — verify yaml field reverts"。然后 step 8 仍要实现 rollback。

- [ ] **Step 8: 实现 yaml rollback**

`card_handlers.rs:336-349` 的 commit failure 分支,在 `state.git_storage.mv(&to_rel, from_rel)` rollback 之外,还要把 yaml field 改回 `None`:

```rust
    if let Err(e) = state.git_storage.add_and_commit_as(
        &[&meta_to, &thread_to],
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Rollback the git mv to leave the working tree clean.
        if let Err(rb_err) = state.git_storage.mv(&to_rel, from_rel) {
            error!("archive_card: rollback mv also failed: {}", rb_err);
        }
        // Rollback the yaml stamp so re-archive next time starts from a clean state.
        meta.archived_via = None;
        if let Ok(rb_yaml) = serde_yaml::to_string(&meta) {
            let _ = std::fs::write(&meta_path, rb_yaml);
        }
        return Response::error(format!(
            "archive_card commit failed: {}; rolled back git mv",
            e
        ));
    }
```

- [ ] **Step 9: Commit**

```bash
git add crates/gitim-daemon/src/card_handlers.rs crates/gitim-daemon/tests/
git commit -m "feat(daemon): stamp archived_via=manual on archive_card

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 2.2: `unarchive_card` 清字段

**Files:**
- Modify: `crates/gitim-daemon/src/card_handlers.rs:378+`(`handle_unarchive_card`)
- Test: 同 Task 2.1 测试文件

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn unarchive_card_clears_archived_via_in_yaml() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let card = repo.create_card("general", "alice", "task").await;
    repo.archive_card("general", &card.card_id, "alice").await.unwrap();

    repo.unarchive_card("general", &card.card_id, "alice").await.unwrap();

    let path = repo.path().join(format!(
        "channels/general/cards/{}/card.meta.yaml",
        card.card_id
    ));
    let yaml = std::fs::read_to_string(&path).expect("active card meta exists");
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(meta.archived_via, None);
    // 也断言 yaml 文件本身不含 archived_via 字段(skip_serializing_if 应起作用)
    assert!(!yaml.contains("archived_via"),
        "expected unset field, got:\n{yaml}");
}
```

- [ ] **Step 2: 跑失败**

```
cargo test -p gitim-daemon unarchive_card_clears_archived_via_in_yaml
```

- [ ] **Step 3: 实现**

`handle_unarchive_card`(line 378 之后)读 yaml 时改 `let` 为 `let mut`,在 permission check 通过后、mv 前加:

```rust
    // After permission check, before git mv:
    meta.archived_via = None;
    let new_yaml = match serde_yaml::to_string(&meta) {
        Ok(s) => s,
        Err(e) => return Response::error(format!("failed to serialize card meta: {}", e)),
    };
    if let Err(e) = std::fs::write(&meta_path, new_yaml) {
        return Response::error(format!("failed to write card meta: {}", e));
    }
```

注:`handle_unarchive_card` 内 `meta_path` 当前指向 `archive/channels/.../card.meta.yaml`(`located.rel_path`)。改后再 git mv 到 active 位置。

同样加 commit-failure rollback(把字段改回原值)。原值取 mv 前的 meta clone:

```rust
    let original_archived_via = meta.archived_via.clone();
    meta.archived_via = None;
    // ... write yaml, git mv, commit ...
    // 失败分支:
    if /* commit failed */ {
        // mv rollback
        meta.archived_via = original_archived_via;
        if let Ok(rb_yaml) = serde_yaml::to_string(&meta) {
            let _ = std::fs::write(&meta_path, rb_yaml);
        }
    }
```

- [ ] **Step 4: 跑测试,确认通过**

```
cargo test -p gitim-daemon unarchive_card_clears_archived_via_in_yaml
```

- [ ] **Step 5: 跑 daemon 全测**

```
cargo test -p gitim-daemon
```

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(daemon): clear archived_via on unarchive_card

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 3: Rust daemon archive_channel / unarchive_channel cards 跟随

### Task 3.1: `archive_channel` 把 cards 一起 mv

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/channel.rs:185-303`(`handle_archive_channel`)
- Test: `crates/gitim-daemon/tests/` 现有 archive_channel 测试文件

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn archive_channel_moves_active_cards_with_archived_via_channel() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let card1 = repo.create_card("general", "alice", "a").await;
    let card2 = repo.create_card("general", "alice", "b").await;

    repo.archive_channel("general", "alice").await.unwrap();

    // 卡片应在 archive/channels/general/cards/<id>/
    for card in [&card1, &card2] {
        let path = repo.path().join(format!(
            "archive/channels/general/cards/{}/card.meta.yaml",
            card.card_id
        ));
        let yaml = std::fs::read_to_string(&path)
            .unwrap_or_else(|_| panic!("archived card meta exists: {}", path.display()));
        let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
        assert_eq!(
            meta.archived_via,
            Some(gitim_core::types::ArchivedVia::Channel),
            "card {} should be archived_via channel",
            card.card_id
        );
    }

    // 原 active 路径不应存在
    assert!(!repo.path().join("channels/general/cards").exists()
        || repo.path().join("channels/general/cards").read_dir().unwrap().count() == 0);
}

#[tokio::test]
async fn archive_channel_does_not_touch_existing_manual_archived_cards() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let manual_card = repo.create_card("general", "alice", "manual").await;
    repo.archive_card("general", &manual_card.card_id, "alice").await.unwrap();

    let auto_card = repo.create_card("general", "alice", "auto").await;
    repo.archive_channel("general", "alice").await.unwrap();

    let manual_yaml = std::fs::read_to_string(repo.path().join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        manual_card.card_id
    ))).unwrap();
    let manual_meta: gitim_core::types::CardMeta = serde_yaml::from_str(&manual_yaml).unwrap();
    assert_eq!(manual_meta.archived_via, Some(gitim_core::types::ArchivedVia::Manual),
        "previously-manual card must keep manual stamp");

    let auto_yaml = std::fs::read_to_string(repo.path().join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        auto_card.card_id
    ))).unwrap();
    let auto_meta: gitim_core::types::CardMeta = serde_yaml::from_str(&auto_yaml).unwrap();
    assert_eq!(auto_meta.archived_via, Some(gitim_core::types::ArchivedVia::Channel));
}
```

- [ ] **Step 2: 跑失败**

```
cargo test -p gitim-daemon archive_channel_moves_active_cards
```

- [ ] **Step 3: 实现 — 列举 active cards、批量改 yaml + mv,合并到一个 commit**

`crates/gitim-daemon/src/handlers/channel.rs`,在 `handle_archive_channel` 的 step 5 创建 archive_dir 之后(line 231)、step 6 mv channel 文件之前(line 233),插入 active cards 处理:

```rust
    // 5b. Discover active cards in channels/<ch>/cards/ — they must follow the channel.
    let active_cards_dir = state
        .repo_root
        .join("channels")
        .join(channel_name.to_string())
        .join("cards");
    let mut card_moves: Vec<(String, String)> = Vec::new(); // (from_rel, to_rel)
    let mut card_files_to_commit: Vec<String> = Vec::new();
    if active_cards_dir.exists() {
        let entries = match std::fs::read_dir(&active_cards_dir) {
            Ok(e) => e,
            Err(e) => return Response::error(format!("failed to read cards dir: {}", e)),
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => return Response::error(format!("failed to read dir entry: {}", e)),
            };
            if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let card_id = entry.file_name().to_string_lossy().to_string();
            let card_dir = entry.path();
            let meta_file = card_dir.join("card.meta.yaml");
            if !meta_file.exists() {
                continue;
            }
            // Stamp archived_via = Channel
            let yaml = match std::fs::read_to_string(&meta_file) {
                Ok(s) => s,
                Err(e) => return Response::error(format!("failed to read card meta {}: {}", card_id, e)),
            };
            let mut meta: gitim_core::types::CardMeta = match serde_yaml::from_str(&yaml) {
                Ok(m) => m,
                Err(e) => return Response::error(format!("failed to parse card meta {}: {}", card_id, e)),
            };
            meta.archived_via = Some(gitim_core::types::ArchivedVia::Channel);
            let new_yaml = match serde_yaml::to_string(&meta) {
                Ok(s) => s,
                Err(e) => return Response::error(format!("failed to serialize card meta {}: {}", card_id, e)),
            };
            if let Err(e) = std::fs::write(&meta_file, new_yaml) {
                return Response::error(format!("failed to write card meta {}: {}", card_id, e));
            }
            let from_rel = format!("channels/{}/cards/{}", channel_name, card_id);
            let to_rel = format!("archive/channels/{}/cards/{}", channel_name, card_id);
            card_moves.push((from_rel.clone(), to_rel.clone()));
            card_files_to_commit.push(format!("{}/card.meta.yaml", to_rel));
            card_files_to_commit.push(format!("{}/discussion.thread", to_rel));
        }
    }
    // Ensure target parent exists
    if !card_moves.is_empty() {
        let archive_cards_dir = state
            .repo_root
            .join("archive")
            .join("channels")
            .join(channel_name.to_string())
            .join("cards");
        if let Err(e) = std::fs::create_dir_all(&archive_cards_dir) {
            return Response::error(format!("failed to create archive cards dir: {}", e));
        }
    }
    for (from_rel, to_rel) in &card_moves {
        if let Err(e) = state.git_storage.mv(from_rel, to_rel) {
            // best-effort rollback of prior mv + yaml
            for (rb_from, rb_to) in card_moves.iter().take_while(|(f, _)| f != from_rel) {
                let _ = state.git_storage.mv(rb_to, rb_from);
            }
            return Response::error(format!("git mv card {} failed: {}", from_rel, e));
        }
    }
```

然后在 step 7(line 247-256)的 `add_and_commit_as` 调用前,把 cards 文件也加入 commit list:

```rust
    // 7. git add + commit (single commit includes channel meta+thread AND all card files)
    let mut commit_paths: Vec<&str> = vec![&thread_to, &meta_to];
    for f in &card_files_to_commit {
        commit_paths.push(f.as_str());
    }
    let commit_msg = format!("archive: #{} by @{}", channel, author);
    let (author_name, author_email) = state.author_for(&author);
    if let Err(e) = state.git_storage.add_and_commit_as(
        &commit_paths,
        &commit_msg,
        Some((&author_name, &author_email)),
    ) {
        // Rollback all card mvs + channel mvs
        for (from_rel, to_rel) in &card_moves {
            let _ = state.git_storage.mv(to_rel, from_rel);
        }
        let _ = state.git_storage.mv(&meta_to, &meta_from);
        let _ = state.git_storage.mv(&thread_to, &thread_from);
        return Response::error(format!("archive commit failed: {}", e));
    }
```

注:`card_files_to_commit` 需要 own `String` 而不是 `&str`,因 commit 前 push retry 期间需要持续有效。代码中 `commit_paths` 通过 `f.as_str()` 借出来即可,只要 `card_files_to_commit` 在 scope 内。

- [ ] **Step 4: 跑测试**

```
cargo test -p gitim-daemon archive_channel_moves_active_cards
cargo test -p gitim-daemon archive_channel_does_not_touch_existing_manual
```

Expected: 都 pass。

- [ ] **Step 5: 跑 daemon 全测**

```
cargo test -p gitim-daemon
```

注意:现有 `archive_channel` 测试可能没建过 cards,行为应保持兼容(空 cards/ 目录 → card_moves 为空 → 跟原行为一致)。验证现有测试仍 green。

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(daemon): archive_channel moves cards subtree with archived_via=channel

Single commit covers channel meta+thread plus all active cards. Manual-
archived cards already under archive/ are not touched. Rollback path
reverts all mvs on commit failure.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 3.2: `unarchive_channel` filter `archived_via == channel`

**Files:**
- Modify: `crates/gitim-daemon/src/handlers/channel.rs:305+`(`handle_unarchive_channel`)
- Test: 同 Task 3.1 测试文件

- [ ] **Step 1: 写失败测试**

```rust
#[tokio::test]
async fn unarchive_channel_restores_only_channel_archived_cards() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let manual_card = repo.create_card("general", "alice", "manual").await;
    repo.archive_card("general", &manual_card.card_id, "alice").await.unwrap();
    let auto_card = repo.create_card("general", "alice", "auto").await;
    repo.archive_channel("general", "alice").await.unwrap();

    repo.unarchive_channel("general", "alice").await.unwrap();

    // auto_card 应回到 active
    let active_meta = repo.path().join(format!(
        "channels/general/cards/{}/card.meta.yaml",
        auto_card.card_id
    ));
    let auto_yaml = std::fs::read_to_string(&active_meta).expect("auto card back to active");
    let auto: gitim_core::types::CardMeta = serde_yaml::from_str(&auto_yaml).unwrap();
    assert_eq!(auto.archived_via, None);
    assert!(!auto_yaml.contains("archived_via"));

    // manual_card 仍留在 archive
    let manual_archived_meta = repo.path().join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        manual_card.card_id
    ));
    let manual_yaml = std::fs::read_to_string(&manual_archived_meta)
        .expect("manual card stays archived");
    let manual: gitim_core::types::CardMeta = serde_yaml::from_str(&manual_yaml).unwrap();
    assert_eq!(manual.archived_via, Some(gitim_core::types::ArchivedVia::Manual));
}
```

- [ ] **Step 2: 跑失败**

```
cargo test -p gitim-daemon unarchive_channel_restores_only_channel
```

- [ ] **Step 3: 实现**

`handle_unarchive_channel` 当前结构(line 305+),先读 archive meta、permission check,然后 mv 文件。改为:

在 permission check 通过后、mv channel 文件之前,扫描 `archive/channels/<ch>/cards/`,filter `archived_via == Channel`,逐个改 yaml(unset archived_via)+ mv,然后 mv channel 自身。

具体代码 pattern 镜像 Task 3.1,只是方向反向 + filter 条件改为 `meta.archived_via == Some(ArchivedVia::Channel)`。

伪代码骨架(详细 fill in 时参照 Task 3.1):

```rust
    // After permission check, before channel mv:
    let archive_cards_dir = state.repo_root
        .join("archive/channels").join(channel_name.to_string()).join("cards");
    let mut card_moves: Vec<(String, String)> = Vec::new();
    let mut commit_paths_owned: Vec<String> = Vec::new();
    if archive_cards_dir.exists() {
        for entry in std::fs::read_dir(&archive_cards_dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() { continue; }
            let card_id = entry.file_name().to_string_lossy().to_string();
            let meta_file = entry.path().join("card.meta.yaml");
            if !meta_file.exists() { continue; }
            let yaml = std::fs::read_to_string(&meta_file)?;
            let mut meta: CardMeta = serde_yaml::from_str(&yaml)?;
            if meta.archived_via != Some(ArchivedVia::Channel) { continue; }
            meta.archived_via = None;
            std::fs::write(&meta_file, serde_yaml::to_string(&meta)?)?;
            let from_rel = format!("archive/channels/{}/cards/{}", channel_name, card_id);
            let to_rel = format!("channels/{}/cards/{}", channel_name, card_id);
            card_moves.push((from_rel.clone(), to_rel.clone()));
            commit_paths_owned.push(format!("{}/card.meta.yaml", to_rel));
            commit_paths_owned.push(format!("{}/discussion.thread", to_rel));
        }
    }
    if !card_moves.is_empty() {
        std::fs::create_dir_all(state.repo_root.join("channels").join(channel_name.to_string()).join("cards"))?;
        for (from_rel, to_rel) in &card_moves {
            state.git_storage.mv(from_rel, to_rel)?;
        }
    }
    // ... 接现有 channel meta + thread mv,把 card files 加入 add_and_commit_as 的 paths ...
```

错误处理 + rollback 沿用 Task 3.1 模式(reverse mv + 恢复 yaml 字段)。所有错误用 `Response::error` 返回,不要 panic / `?`(handler 不能用 `?`)。

- [ ] **Step 4: 跑测试**

```
cargo test -p gitim-daemon unarchive_channel_restores_only_channel
```

- [ ] **Step 5: 跑 daemon 全测**

```
cargo test -p gitim-daemon
```

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(daemon): unarchive_channel restores only archived_via=channel cards

Manual-archived cards stay under archive/ even after channel returns
active, matching the design's 'cards self-record archive provenance'
invariant.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 4: Rust daemon reconcile_orphan_cards

### Task 4.1: `reconcile_orphan_cards` 函数

**Files:**
- Create: `crates/gitim-daemon/src/reconcile.rs`
- Modify: `crates/gitim-daemon/src/lib.rs`(`pub mod reconcile;`)
- Modify: `crates/gitim-daemon/src/state.rs` 或 `AppState::new` 调用点(具体位置由 codebase 当前结构决定 —— 执行前 `rg "fn new" crates/gitim-daemon/src/state.rs`)

- [ ] **Step 1: 写失败测试**

`crates/gitim-daemon/tests/reconcile.rs`:

```rust
use gitim_daemon::reconcile::reconcile_orphan_cards;
mod common;
use common::TestRepo;

#[tokio::test]
async fn reconcile_moves_orphan_card_dir_to_archive() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let card = repo.create_card("general", "alice", "task").await;

    // 模拟 legacy archive_channel(只 mv channel meta+thread)
    let active_meta = repo.path().join("channels/general.meta.yaml");
    let active_thread = repo.path().join("channels/general.thread");
    let archive_meta = repo.path().join("archive/channels/general.meta.yaml");
    let archive_thread = repo.path().join("archive/channels/general.thread");
    std::fs::create_dir_all(repo.path().join("archive/channels")).unwrap();
    std::fs::rename(&active_meta, &archive_meta).unwrap();
    std::fs::rename(&active_thread, &archive_thread).unwrap();
    // 此时 channels/general/cards/<id>/ 仍在原位(孤儿)

    let n_migrated = reconcile_orphan_cards(repo.state()).await.unwrap();
    assert_eq!(n_migrated, 1);

    // 卡片应已迁移
    let migrated_yaml = repo.path().join(format!(
        "archive/channels/general/cards/{}/card.meta.yaml",
        card.card_id
    ));
    assert!(migrated_yaml.exists(), "card meta migrated to archive");
    let yaml = std::fs::read_to_string(&migrated_yaml).unwrap();
    let meta: gitim_core::types::CardMeta = serde_yaml::from_str(&yaml).unwrap();
    assert_eq!(meta.archived_via, Some(gitim_core::types::ArchivedVia::Channel));

    // 原孤儿目录应已清(允许保留空 channels/general/ 父目录,git 不跟踪空目录)
    let orphan_card = repo.path().join(format!(
        "channels/general/cards/{}",
        card.card_id
    ));
    assert!(!orphan_card.exists());
}

#[tokio::test]
async fn reconcile_is_idempotent_when_no_orphans() {
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    repo.create_card("general", "alice", "task").await;
    let head_before = repo.head_commit();

    let n = reconcile_orphan_cards(repo.state()).await.unwrap();
    assert_eq!(n, 0);

    let head_after = repo.head_commit();
    assert_eq!(head_before, head_after, "no-op when no orphans");
}

#[tokio::test]
async fn reconcile_skips_active_channels_with_cards() {
    // 正常 active channel + cards 不应被误迁移
    let repo = TestRepo::new("alice").await;
    repo.create_channel("general", "alice").await;
    let card = repo.create_card("general", "alice", "task").await;
    let head_before = repo.head_commit();

    let n = reconcile_orphan_cards(repo.state()).await.unwrap();
    assert_eq!(n, 0);

    assert!(repo.path().join(format!(
        "channels/general/cards/{}/card.meta.yaml",
        card.card_id
    )).exists());
    assert_eq!(repo.head_commit(), head_before);
}
```

- [ ] **Step 2: 跑失败**

```
cargo test -p gitim-daemon --test reconcile
```

- [ ] **Step 3: 实现 reconcile.rs**

```rust
// crates/gitim-daemon/src/reconcile.rs
use crate::state::SharedState;
use gitim_core::types::{ArchivedVia, CardMeta};
use tracing::{info, warn};

/// Scan `channels/<ch>/cards/` directories whose corresponding channel meta
/// has moved to `archive/channels/<ch>.meta.yaml` (legacy archive_channel
/// orphans). For each orphan card:
///   - stamp `archived_via: channel` in its yaml
///   - git mv it from `channels/<ch>/cards/<id>` to `archive/channels/<ch>/cards/<id>`
/// All moves committed as a single commit by `system@gitim`. Returns the
/// number of cards migrated (0 ⇒ no commit, no push).
pub async fn reconcile_orphan_cards(state: SharedState) -> Result<usize, String> {
    let repo_root = &state.repo_root;
    let channels_dir = repo_root.join("channels");
    if !channels_dir.exists() {
        return Ok(0);
    }

    let mut card_moves: Vec<(String, String)> = Vec::new();
    let mut commit_paths: Vec<String> = Vec::new();

    let entries = std::fs::read_dir(&channels_dir)
        .map_err(|e| format!("read channels/: {}", e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("read entry: {}", e))?;
        let ft = entry.file_type().map_err(|e| format!("file_type: {}", e))?;
        if !ft.is_dir() {
            continue; // only handle directory entries (channels/<ch>/)
        }
        let channel_name = entry.file_name().to_string_lossy().to_string();
        let active_meta = channels_dir.join(format!("{}.meta.yaml", channel_name));
        let archive_meta = repo_root
            .join("archive/channels")
            .join(format!("{}.meta.yaml", channel_name));
        if active_meta.exists() || !archive_meta.exists() {
            continue; // not an orphan — channel still active, or never archived
        }
        let cards_dir = entry.path().join("cards");
        if !cards_dir.exists() {
            continue;
        }

        let card_entries = std::fs::read_dir(&cards_dir)
            .map_err(|e| format!("read {}/cards: {}", channel_name, e))?;
        for card_entry in card_entries {
            let card_entry = card_entry.map_err(|e| format!("read card entry: {}", e))?;
            if !card_entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            let card_id = card_entry.file_name().to_string_lossy().to_string();
            let meta_path = card_entry.path().join("card.meta.yaml");
            if !meta_path.exists() {
                continue;
            }
            let yaml = std::fs::read_to_string(&meta_path)
                .map_err(|e| format!("read card meta {}: {}", card_id, e))?;
            let mut meta: CardMeta = serde_yaml::from_str(&yaml)
                .map_err(|e| format!("parse card meta {}: {}", card_id, e))?;
            meta.archived_via = Some(ArchivedVia::Channel);
            let new_yaml = serde_yaml::to_string(&meta)
                .map_err(|e| format!("serialize card meta {}: {}", card_id, e))?;
            std::fs::write(&meta_path, new_yaml)
                .map_err(|e| format!("write card meta {}: {}", card_id, e))?;

            let from_rel = format!("channels/{}/cards/{}", channel_name, card_id);
            let to_rel = format!("archive/channels/{}/cards/{}", channel_name, card_id);
            card_moves.push((from_rel.clone(), to_rel.clone()));
            commit_paths.push(format!("{}/card.meta.yaml", to_rel));
            commit_paths.push(format!("{}/discussion.thread", to_rel));
        }
    }

    if card_moves.is_empty() {
        return Ok(0);
    }

    // Ensure target parent dirs exist + git mv each card
    for (from_rel, to_rel) in &card_moves {
        let to_parent = repo_root
            .join(to_rel)
            .parent()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| format!("invalid to_rel: {}", to_rel))?;
        std::fs::create_dir_all(&to_parent)
            .map_err(|e| format!("mkdir {}: {}", to_parent.display(), e))?;
        state.git_storage.mv(from_rel, to_rel)
            .map_err(|e| format!("git mv {} -> {}: {}", from_rel, to_rel, e))?;
    }

    // Single commit, system author
    let path_refs: Vec<&str> = commit_paths.iter().map(|s| s.as_str()).collect();
    let commit_msg = "chore: reconcile orphan cards under archived channels";
    state.git_storage.add_and_commit_as(
        &path_refs,
        commit_msg,
        Some(("system", "system@gitim")),
    ).map_err(|e| format!("reconcile commit failed: {}", e))?;

    // Best-effort push; failure here is non-fatal (sync_loop will retry on next cycle)
    if state.git_storage.has_remote() {
        if let Err(e) = state.git_storage.push() {
            warn!("reconcile push failed (will retry via sync_loop): {}", e);
        }
    }

    info!("reconcile: migrated {} orphan cards to archive", card_moves.len());
    Ok(card_moves.len())
}
```

注:具体 `state.git_storage` API、`SharedState` 字段、`author_for` 等接口取决于现有 daemon 结构,实施时按真实 API 适配。`add_and_commit_as` 是参照 archive_channel/archive_card 已用方式 — 若签名不同需调整。

- [ ] **Step 4: 注册模块 + 启动调用**

`crates/gitim-daemon/src/lib.rs` 加 `pub mod reconcile;`

定位 daemon 启动入口(通常是 `AppState::new` 之后,或 `main.rs` 中 `tokio::spawn` 之前的 init 阶段):

```bash
rg "AppState::new\b|fn run_daemon\b" crates/gitim-daemon/src/
```

在 init 阶段末尾、handler loop 启动前,加调用:

```rust
    // Reconcile any legacy orphan cards from pre-2026-05 archive_channel
    // implementations that only moved channel meta+thread.
    if let Err(e) = reconcile::reconcile_orphan_cards(state.clone()).await {
        tracing::error!("reconcile_orphan_cards failed at boot: {}", e);
        // non-fatal — proceed to handler loop; sync_loop will pick up later
    }
```

- [ ] **Step 5: 跑测试**

```
cargo test -p gitim-daemon --test reconcile
```

Expected: 3 passed。

- [ ] **Step 6: 跑 daemon 全测**

```
cargo test -p gitim-daemon
```

特别留意 daemon 启动 integration test 是否过(reconcile 会在每个测试 daemon 启动时跑一次)。无孤儿场景应 no-op。

- [ ] **Step 7: Commit**

```bash
git add crates/gitim-daemon/src/reconcile.rs crates/gitim-daemon/src/lib.rs crates/gitim-daemon/src/state.rs crates/gitim-daemon/tests/reconcile.rs
git commit -m "feat(daemon): reconcile orphan cards on boot

Legacy archive_channel only moved channel meta+thread, leaving
channels/<ch>/cards/ as orphans. Boot-time scan migrates them to
archive/channels/<ch>/cards/ with archived_via=channel stamped.
Single commit, idempotent — empty result skips commit.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 5: Frontend types + daemon-web 行为

### Task 5.1: 加 `archived_via` 到 `Card` interface

**Files:**
- Modify: `products/gitim/frontend/src/lib/types.ts:192-202`(`Card` interface)
- Test: 无(纯类型,无运行时)

- [ ] **Step 1: 编辑**

```typescript
export type ArchivedVia = "channel" | "manual";

export interface Card {
  card_id: string;
  channel: string;
  title: string;
  status: CardStatus;
  labels: string[];
  assignee: string | null;
  created_by: string;
  created_at: string;
  updated_at: string;
  archived_via?: ArchivedVia;
}
```

- [ ] **Step 2: 跑 typecheck**

```bash
cd products/gitim/frontend && pnpm exec tsc -b
```

Expected: 0 errors。可能有 `Card` 构造点 strict-mode 不允许 unknown property — 那是好事,本任务暴露的就是该字段会被流转到的位置。修这些位置:把 `archived_via: undefined`(active 卡片)或对应值塞入。常见位置:`daemon-web/handlers.ts` 的 `RawCardMeta`、`createCard`、`readCardMeta`。

- [ ] **Step 3: Commit**

```bash
git add products/gitim/frontend/src/lib/types.ts products/gitim/frontend/src/daemon-web/handlers.ts
git commit -m "feat(frontend): add archived_via to Card type

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 5.2: `archiveCard` / `unarchiveCard` 改 yaml

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`(`archiveCard` at line 1302+,`unarchiveCard` at line 1330+)
- Test: `products/gitim/frontend/src/daemon-web/handlers.test.ts`(扩展现有 archive_card test 在 line 1156+)

- [ ] **Step 1: 写失败测试**

在 handlers.test.ts 现有 archive_card describe block 加:

```typescript
it("stamps archived_via=manual in yaml on archiveCard", async () => {
  await archiveCard("general", "20260317-120000-abc");
  const yaml = files.get(
    "/repo/archive/channels/general/cards/20260317-120000-abc/card.meta.yaml"
  )!;
  expect(yaml).toContain("archived_via: manual");
});

it("clears archived_via in yaml on unarchiveCard", async () => {
  await archiveCard("general", "20260317-120000-abc");
  await unarchiveCard("general", "20260317-120000-abc");
  const yaml = files.get(
    "/repo/channels/general/cards/20260317-120000-abc/card.meta.yaml"
  )!;
  expect(yaml).not.toContain("archived_via");
});
```

- [ ] **Step 2: 跑失败**

```
pnpm test -- handlers.test
```

- [ ] **Step 3: 实现**

定位 `archiveCard` 函数(line 1302+),在 read yaml + permission check 之后、git mv 之前加字段写:

```typescript
// 在 await readCardMeta(...) 拿到 card 之后
const updatedYaml = stringifyCardMeta({ ...card, archived_via: "manual" }) as string;
await writeFile(`${located.absDir}/card.meta.yaml`, updatedYaml);
// 然后接现有 mv 逻辑
```

同样 `unarchiveCard`(line 1330+):

```typescript
// 在 readCardMeta 之后
const cardWithoutMark = { ...card };
delete cardWithoutMark.archived_via;
const updatedYaml = stringifyCardMeta(cardWithoutMark) as string;
await writeFile(`${located.absDir}/card.meta.yaml`, updatedYaml);
// 然后接现有 mv 逻辑
```

`stringifyCardMeta` 是现有 helper(`createCard` 用过),sequentialize yaml。具体 stringify 行为:`undefined` 字段是否被 yaml lib 输出 `null`?确认:如果输出 `archived_via: null`,后端 / reconcile 也应当兼容(等价于 absent)。但更干净的方式是 `delete` 之后传给 stringify,让字段完全不出现。`stringifyCardMeta` 内若用 `yaml.dump` 选项 `noRefs/skipInvalid`,默认 undefined 字段会被 omit。验证:Step 1 测试断言 `not.toContain("archived_via")` — pass 即可。

- [ ] **Step 4: 跑测试**

```
pnpm test -- handlers.test
```

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(frontend): stamp/clear archived_via in daemon-web card archive ops

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 5.3: `archiveChannel` 把 cards 一起 mv

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts:672-714`(`archiveChannel`)
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts:1641-1673`(`moveChannelFiles` 或者新建 helper)
- Test: handlers.test.ts archive channel describe block

- [ ] **Step 1: 写失败测试**

```typescript
it("moves cards subtree under archived channel and stamps archived_via=channel", async () => {
  // 假设 fixture 已 setup general channel + 2 active cards
  await archiveChannel("general");

  // active cards 路径应为空
  expect(files.has("/repo/channels/general/cards/CARD1/card.meta.yaml")).toBe(false);
  expect(files.has("/repo/channels/general/cards/CARD2/card.meta.yaml")).toBe(false);

  // archive 路径应有
  const yaml1 = files.get("/repo/archive/channels/general/cards/CARD1/card.meta.yaml")!;
  const yaml2 = files.get("/repo/archive/channels/general/cards/CARD2/card.meta.yaml")!;
  expect(yaml1).toContain("archived_via: channel");
  expect(yaml2).toContain("archived_via: channel");
});

it("does not touch existing manual-archived cards on channel archive", async () => {
  // fixture: MANUAL_CARD 已通过 archiveCard 进 archive,AUTO_CARD 仍 active
  await archiveChannel("general");
  const manualYaml = files.get(
    "/repo/archive/channels/general/cards/MANUAL_CARD/card.meta.yaml"
  )!;
  expect(manualYaml).toContain("archived_via: manual"); // 不被覆盖
});
```

测试 fixture 取决于现有 test infrastructure。参考 line 1156 现有 `archives active cards into archive/channels` 测试的 setup pattern。

- [ ] **Step 2: 跑失败**

```
pnpm test -- handlers.test
```

- [ ] **Step 3: 实现 — 在 archiveChannel 内列举 + 写 yaml + mv 整体**

`archiveChannel`(line 672)改为:

```typescript
export async function archiveChannel(channel: string): Promise<ApiResponse> {
  try {
    const s = getState();
    if (!s.token) return reconnectRequired();
    const invalidChannel = validateChannelName(channel);
    if (invalidChannel) return err(invalidChannel);

    const metaRelPath = channelMetaPath(channel);
    const metaAbsPath = `${s.repoDir}/${metaRelPath}`;
    if (!(await exists(metaAbsPath))) {
      return err(`channel '${channel}' does not exist`);
    }

    const meta = parseYaml(await readFile(metaAbsPath)) as unknown as ChannelMeta;
    if (meta.created_by !== s.me.handler) {
      return err("only channel creator can archive");
    }

    const archiveMetaPath = `${s.repoDir}/archive/channels/${channel}.meta.yaml`;
    if (await exists(archiveMetaPath)) {
      return err(`channel '${channel}' is already archived`);
    }

    // Discover active cards under channels/<ch>/cards/
    const activeCardsDir = `${s.repoDir}/channels/${channel}/cards`;
    type CardMove = {
      cardId: string;
      fromRel: string;
      toRel: string;
      metaFiles: string[];   // committed paths
      threadFiles: string[];
    };
    const cardMoves: CardMove[] = [];
    if (await exists(activeCardsDir)) {
      const cardIds = await readdir(activeCardsDir);
      for (const cardId of cardIds) {
        const cardDir = `${activeCardsDir}/${cardId}`;
        const cardMetaPath = `${cardDir}/card.meta.yaml`;
        if (!(await exists(cardMetaPath))) continue;
        const cardYaml = await readFile(cardMetaPath);
        const card = parseYaml(cardYaml) as Card;
        const stamped = { ...card, archived_via: "channel" as const };
        await writeFile(cardMetaPath, stringifyCardMeta(stamped) as string);
        cardMoves.push({
          cardId,
          fromRel: `channels/${channel}/cards/${cardId}`,
          toRel: `archive/channels/${channel}/cards/${cardId}`,
          metaFiles: [
            `archive/channels/${channel}/cards/${cardId}/card.meta.yaml`,
          ],
          threadFiles: [
            `archive/channels/${channel}/cards/${cardId}/discussion.thread`,
          ],
        });
      }
    }

    // Ensure target dirs exist
    if (cardMoves.length > 0) {
      await mkdirp(`${s.repoDir}/archive/channels/${channel}/cards`);
      for (const m of cardMoves) {
        // git mv via filesystem rename (isomorphic-git tracks via add/remove)
        const fromAbs = `${s.repoDir}/${m.fromRel}`;
        const toAbs = `${s.repoDir}/${m.toRel}`;
        await renameRecursive(fromAbs, toAbs);
      }
    }

    // Move channel meta + thread (existing helper, but inline + extend its commit)
    await mkdirp(`${s.repoDir}/archive/channels`);
    const channelFromMeta = metaRelPath;
    const channelFromThread = `channels/${channel}.thread`;
    const channelToMeta = `archive/channels/${channel}.meta.yaml`;
    const channelToThread = `archive/channels/${channel}.thread`;
    await renameFile(`${s.repoDir}/${channelFromMeta}`, `${s.repoDir}/${channelToMeta}`);
    await renameFile(`${s.repoDir}/${channelFromThread}`, `${s.repoDir}/${channelToThread}`);

    // Single commit
    const adds = [
      channelToMeta,
      channelToThread,
      ...cardMoves.flatMap((m) => [...m.metaFiles, ...m.threadFiles]),
    ];
    const removes = [
      channelFromMeta,
      channelFromThread,
      ...cardMoves.flatMap((m) => [
        `${m.fromRel}/card.meta.yaml`,
        `${m.fromRel}/discussion.thread`,
      ]),
    ];
    await gitOps.addRemoveAndCommit(
      s.repoDir,
      adds,
      removes,
      `archive: #${channel} by @${s.me.handler}`,
      s.me.handler,
    );

    await refreshChannelsCache();
    const sync = await syncAfterCommit();
    return ok({ channel, archived_by: s.me.handler, ...sync });
  } catch (e) {
    return err(String((e as Error).message ?? e));
  }
}
```

辅助函数 `renameRecursive`、`renameFile`:如果现有代码没有,定位 worker / isomorphic-git fs wrapper 看现有 mv 怎么做(`moveChannelFiles` line 1641 是参考)。可能需要 read + write + remove pattern(isomorphic-git 不支持原子 rename),具体如下:

```typescript
async function renameRecursive(fromAbs: string, toAbs: string): Promise<void> {
  await mkdirp(parentPath(toAbs));
  // 单层目录,文件挨个 read/write/remove。对于 cards/<id>/ 通常只 2 个文件。
  const items = await readdir(fromAbs);
  for (const item of items) {
    const fromItem = `${fromAbs}/${item}`;
    const toItem = `${toAbs}/${item}`;
    if ((await statIsFile(fromItem))) {
      const content = await readFile(fromItem);
      await mkdirp(parentPath(toItem));
      await writeFile(toItem, content);
      await removeTrackedFile(fromItem);
    }
  }
}
```

`addRemoveAndCommit` 在 `gitOps`(line 705 现有 archiveChannel 调过)已暴露。`moveChannelFiles` 现有调用方继续保留(unarchive 也用),但 archive_channel 改用 inline + extended add/remove。

注意保留现有 `refreshChannelsCache` + `syncAfterCommit` 调用,这是 polling fix 之后整个 invalidation 链路。

- [ ] **Step 4: 跑测试**

```
pnpm test -- handlers.test
```

Expected: 新测试 + 现有 archiveChannel 测试都过。

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(frontend): archiveChannel moves cards subtree with archived_via=channel

Single commit covers channel meta+thread plus all active card files.
Cards already in archive/ (via archiveCard) are not touched.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 5.4: `unarchiveChannel` filter `archived_via=channel`

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts:716-758`(`unarchiveChannel`)
- Test: handlers.test.ts unarchive describe block

- [ ] **Step 1: 写失败测试**

```typescript
it("only restores cards with archived_via=channel on unarchiveChannel", async () => {
  // fixture: MANUAL_CARD archived via archiveCard, AUTO_CARD archived via archiveChannel
  await archiveChannel("general");  // stamps AUTO_CARD with channel; MANUAL stays manual
  await unarchiveChannel("general");

  const auto = files.get("/repo/channels/general/cards/AUTO_CARD/card.meta.yaml")!;
  expect(auto).not.toContain("archived_via");

  // MANUAL_CARD 仍留 archive
  expect(files.has("/repo/channels/general/cards/MANUAL_CARD/card.meta.yaml")).toBe(false);
  const manual = files.get(
    "/repo/archive/channels/general/cards/MANUAL_CARD/card.meta.yaml"
  )!;
  expect(manual).toContain("archived_via: manual");
});
```

- [ ] **Step 2: 跑失败**

```
pnpm test -- handlers.test
```

- [ ] **Step 3: 实现**

镜像 Task 5.3:在 `unarchiveChannel` 内,在 mv channel meta+thread 前,扫 `archive/channels/<ch>/cards/`,filter `archived_via === "channel"`,逐个 unset 字段 + mv 回 active 位置。整合到同一 `addRemoveAndCommit` call。

- [ ] **Step 4: 跑测试**

```
pnpm test -- handlers.test
```

- [ ] **Step 5: Commit**

```bash
git commit -am "feat(frontend): unarchiveChannel restores only archived_via=channel cards

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 5.5: `reconcileOrphanCards` + worker boot 调用

**Files:**
- Modify: `products/gitim/frontend/src/daemon-web/handlers.ts`(末尾追加函数)
- Modify: `products/gitim/frontend/src/daemon-web/worker.ts:1-50`(boot 调用)
- Test: handlers.test.ts 新增 reconcile describe block

- [ ] **Step 1: 写失败测试**

```typescript
describe("reconcileOrphanCards", () => {
  it("migrates orphan card dirs under archived channels and stamps archived_via=channel", async () => {
    // Setup: channel meta in archive/ but cards subtree still in channels/
    files.set("/repo/archive/channels/general.meta.yaml", "...");
    files.set("/repo/archive/channels/general.thread", "");
    files.set("/repo/channels/general/cards/ORPHAN/card.meta.yaml",
      "title: t\nchannel: general\nstatus: todo\nlabels: []\nassignee: null\ncreated_by: alice\ncreated_at: '2026-01-01T00:00:00Z'\nupdated_at: '2026-01-01T00:00:00Z'\n"
    );
    files.set("/repo/channels/general/cards/ORPHAN/discussion.thread", "");

    const n = await reconcileOrphanCards();
    expect(n).toBe(1);

    expect(files.has("/repo/channels/general/cards/ORPHAN/card.meta.yaml")).toBe(false);
    const yaml = files.get("/repo/archive/channels/general/cards/ORPHAN/card.meta.yaml")!;
    expect(yaml).toContain("archived_via: channel");
  });

  it("is no-op when no orphans (no commit)", async () => {
    files.set("/repo/channels/general.meta.yaml", "...");
    files.set("/repo/channels/general/cards/ACTIVE/card.meta.yaml", "...");
    const commitsBefore = commitLog.length;
    const n = await reconcileOrphanCards();
    expect(n).toBe(0);
    expect(commitLog.length).toBe(commitsBefore);
  });
});
```

- [ ] **Step 2: 跑失败**

```
pnpm test -- handlers.test
```

- [ ] **Step 3: 实现 reconcileOrphanCards**

handlers.ts 末尾追加:

```typescript
/**
 * Boot-time idempotent migration of legacy orphan card dirs.
 *
 * Pre-2026-05 archiveChannel only moved channel meta+thread, leaving
 * channels/<ch>/cards/ as orphans. This scans for that pattern and
 * brings the cards subtree under archive/, stamping archived_via=channel.
 *
 * Returns the number of cards migrated. 0 ⇒ no commit, no push.
 */
export async function reconcileOrphanCards(): Promise<number> {
  await ensureWasmReady();
  const s = getState();
  const channelsDir = `${s.repoDir}/channels`;
  if (!(await exists(channelsDir))) return 0;

  type Move = { from: string; to: string };
  const moves: Move[] = [];
  const adds: string[] = [];
  const removes: string[] = [];

  const items = await readdir(channelsDir);
  for (const item of items) {
    if (item.endsWith(".meta.yaml")) continue; // top-level channel meta files
    const channelName = item;
    const activeMeta = `${channelsDir}/${channelName}.meta.yaml`;
    const archiveMeta = `${s.repoDir}/archive/channels/${channelName}.meta.yaml`;
    if (await exists(activeMeta)) continue;
    if (!(await exists(archiveMeta))) continue;
    const cardsDir = `${channelsDir}/${channelName}/cards`;
    if (!(await exists(cardsDir))) continue;

    const cardIds = await readdir(cardsDir);
    for (const cardId of cardIds) {
      const metaPath = `${cardsDir}/${cardId}/card.meta.yaml`;
      if (!(await exists(metaPath))) continue;
      const yaml = await readFile(metaPath);
      const card = parseYaml(yaml) as Card;
      const stamped = { ...card, archived_via: "channel" as const };
      await writeFile(metaPath, stringifyCardMeta(stamped) as string);

      const fromRel = `channels/${channelName}/cards/${cardId}`;
      const toRel = `archive/channels/${channelName}/cards/${cardId}`;
      await mkdirp(`${s.repoDir}/${toRel}`);
      // Move both files
      const metaContent = await readFile(`${s.repoDir}/${fromRel}/card.meta.yaml`);
      const threadContent = await readFile(`${s.repoDir}/${fromRel}/discussion.thread`);
      await writeFile(`${s.repoDir}/${toRel}/card.meta.yaml`, metaContent);
      await writeFile(`${s.repoDir}/${toRel}/discussion.thread`, threadContent);
      await removeFile(`${s.repoDir}/${fromRel}/card.meta.yaml`);
      await removeFile(`${s.repoDir}/${fromRel}/discussion.thread`);

      adds.push(`${toRel}/card.meta.yaml`, `${toRel}/discussion.thread`);
      removes.push(`${fromRel}/card.meta.yaml`, `${fromRel}/discussion.thread`);
      moves.push({ from: fromRel, to: toRel });
    }
  }

  if (moves.length === 0) return 0;

  await gitOps.addRemoveAndCommit(
    s.repoDir,
    adds,
    removes,
    "chore: reconcile orphan cards under archived channels",
    "system",  // author handler — daemon convention uses "system" for housekeeping commits
  );

  // sync_loop will push next cycle; reconcile itself does not block on push
  return moves.length;
}
```

- [ ] **Step 4: 注册到 worker boot**

`products/gitim/frontend/src/daemon-web/worker.ts`,在 setState / handler 注册流程之后、消息 loop 启动之前,加调用:

```typescript
// After workspace setup, before message handler loop
try {
  const n = await handlers.reconcileOrphanCards();
  if (n > 0) {
    console.log(`[gitim] reconcile: migrated ${n} orphan cards`);
  }
} catch (e) {
  console.warn("[gitim] reconcile failed (non-fatal):", e);
}
```

具体调用位置取决于 worker.ts 现有 init 阶段。如果 worker.ts 没有显式 boot init(各 handler 是 on-demand),改为在第一次有效操作前调用,或暴露 `init()` API 让 app.tsx 显式调一次。

- [ ] **Step 5: 跑测试**

```
pnpm test -- handlers.test
```

Expected: reconcile 两个测试通过 + 其他全 green。

- [ ] **Step 6: Commit**

```bash
git commit -am "feat(frontend): reconcile orphan cards on daemon-web boot

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

---

## Phase 6: 文档 + 终局 baseline

### Task 6.1: 更新 CLAUDE.md Current Orientation

**Files:**
- Modify: `CLAUDE.md`(Current Orientation 段落)

- [ ] **Step 1: 编辑**

在 "Where we are" 后追加一段:

```
**Cards 跟随 channel archive**已落地:archive_channel 把 channels/<ch>/cards/
整个子目录 mv 到 archive/channels/<ch>/cards/,同时把每张卡的
card.meta.yaml 标记 archived_via=channel。unarchive 时按字段筛选,
只复活 archived_via=channel 的卡片;archive_card 单独归档的(archived_via=manual)
保持留在 archive。Rust daemon 和 frontend daemon-web 双端一致。
启动时跑 reconcile_orphan_cards 一次,把 pre-2026-05 archive_channel
留下的孤儿 channels/<archived>/cards/ 自动迁移到正确位置(幂等,无孤儿则 no-op,
不 commit)。
```

更新 **Tensions** 段,把"archive_channel 只 mv 两个文件,留下孤儿 cards 目录"那条删掉(若有)。

- [ ] **Step 2: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update orientation for cards-follow-channel-archive

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>"
```

### Task 6.2: 终局 baseline 全量

- [ ] **Step 1: Rust workspace 全量**

```
cargo test --workspace
```

Expected: 跟 Task 0.1 baseline 比对,新增测试全 green,无 regression。

- [ ] **Step 2: Frontend 全量**

```
cd products/gitim/frontend && pnpm exec tsc -b && pnpm test
```

Expected: 0 typecheck errors,全 vitest 通过。

- [ ] **Step 3: 手动 sanity check(可选,需 user 配合)**

在真实仓库:
- archive 一个有 cards 的 channel,看 webui kanban 立刻不显示这些卡;`git status` 看到所有 cards 子目录被 mv 到 archive,yaml 字段标记 channel;一个 commit。
- unarchive 同 channel,看卡片重新出现,yaml 字段被清。
- archive_card 一张卡,然后 archive_channel,看 manual 那张 yaml 仍保留 manual 字段。
- unarchive_channel,看 manual 那张留 archive,channel 那批回 active。

- [ ] **Step 4: 没问题就 push branch + 开 PR**

```bash
git push -u origin claude/sweet-chatelet-7eff6a
gh pr create --title "feat: cards follow channel archive" --body "$(cat <<'EOF'
## Summary
- Channel archive moves cards subtree (channels/<ch>/cards/) into
  archive/channels/<ch>/cards/, stamping archived_via=channel on each
  card.meta.yaml. Single commit per archive op.
- Channel unarchive only restores cards stamped archived_via=channel;
  manual-archived cards stay under archive/.
- Boot-time reconcile_orphan_cards migrates legacy orphan dirs
  (idempotent, no commit when no orphans).
- Rust daemon and frontend daemon-web both updated.

See [00-design.md](docs/plans/cards-follow-channel-archive/00-design.md) for full design.

## Test plan
- [x] cargo test --workspace (new tests + no regressions)
- [x] pnpm test in products/gitim/frontend (new tests + no regressions)
- [ ] Manual: archive channel with cards in real repo, verify webui
- [ ] Manual: unarchive preserves manual-archived cards

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
)"
```

---

## Self-Review Checklist

执行人(或 plan 作者本人)在 plan 完成后过这个 checklist:

- [ ] **Spec coverage**: 00-design.md 每个决策(D1-D5)、每个 section(3.1-3.2, 4.1-4.6, 5.1-5.4, 6, 7, 8)都对应到 plan 里的 Task。
- [ ] **Placeholder scan**: 无 TBD/TODO/"implement later"。具体代码全 inline,无 "见上面"/"类似 Task X"。
- [ ] **Type consistency**: `ArchivedVia` enum、`archived_via` 字段名、`stringifyCardMeta` helper 名跨 Task 一致。
- [ ] **PR ordering**: Phase 1-4(Rust)→ Phase 5(frontend)→ Phase 6(docs)。frontend / Rust 可以分两 PR 落,在 Phase 5 之前停一次 PR 也合理 —— 由执行人决定。
