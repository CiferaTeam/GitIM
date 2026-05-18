# Workspace Clippy Lints + Pre-commit Hook Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 把散落在 8 个 crate 的 `#![deny(warnings)]` 纪律统一到 `[workspace.lints]`,加 `clippy::all = "deny"` + 关键纪律 lint,并装 pre-commit hook 在 `cargo fmt --check` 和 `cargo clippy --workspace` 失败时拒绝 commit。

**Architecture:** 单一 `[workspace.lints]` 配置点 + 每 crate `[lints] workspace = true` 继承 + 删 per-crate `#![deny(warnings)]`。Hook 源文件版本化在 `scripts/hooks/pre-commit`,`scripts/install-hooks.sh` 用 `git rev-parse --git-common-dir` symlink 到共享 hooks dir(worktree-safe)。

**Tech Stack:** Rust 2021 / Cargo workspace lints (Rust 1.74+) / bash / git hooks。

---

## File Structure

- **Modify**: `Cargo.toml` — 加 `[workspace.lints.rust]` + `[workspace.lints.clippy]`
- **Modify**: 10 个 crate 的 `Cargo.toml` — 末尾加 `[lints] workspace = true`
- **Modify**: 8 个 crate 的 `lib.rs` / `main.rs` — 删 `#![deny(warnings)]`
- **Modify**: `crates/gitim-index/src/lib.rs` — 修 4 个 `manual_flatten`(lines 262, 294, 437, 469)
- **Modify**: `crates/gitim-client/src/client.rs` — 加 `#[allow(clippy::too_many_arguments)]` 到 search()
- **Create**: `scripts/hooks/pre-commit` — 版本化的 hook 源文件
- **Create**: `scripts/install-hooks.sh` — 安装器
- **Modify**: `CLAUDE.md` — 加 hook 安装提示

---

## Task 1: 修 `gitim-index` 4 处 `manual_flatten`

**Files:**
- Modify: `crates/gitim-index/src/lib.rs:262-263, 294-295, 437-438, 469-470`

4 处代码模式完全一致 — `for entry in std::fs::read_dir(...).into_iter().flatten() { if let Ok(entry) = entry { ... } }`。修法是把外层 `.flatten()` 改成 `.flatten().flatten()`,删掉内层 `if let Ok(entry) = entry { }` 包裹层。语义不变(都是丢弃 read_dir 失败和 dir entry 失败)。

- [ ] **Step 1: 看现状,确认 4 处都是同一模式**

Run: `grep -n "if let Ok(entry) = entry" crates/gitim-index/src/lib.rs`

Expected: 4 行匹配,行号 263 / 295 / 438 / 470。

- [ ] **Step 2: 修第一处(line 262-263 区域)**

Edit `crates/gitim-index/src/lib.rs`,找:

```rust
            for entry in std::fs::read_dir(&channels_dir).into_iter().flatten() {
                if let Ok(entry) = entry {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.ends_with(".thread") {
```

替换成:

```rust
            for entry in std::fs::read_dir(&channels_dir).into_iter().flatten().flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.ends_with(".thread") {
```

并删掉对应的多余 `}`(`if let Ok(entry) = entry { }` 的闭合)。最小 diff:删 `if let Ok(entry) = entry {` 这一行,删对应 closing `}`,把外层 for 改 `.flatten().flatten()`。indent 整段往左 4 空格。

- [ ] **Step 3: 修第二处(line 294-295)**

同上模式,变量是 `dm_dir`,索引类型 `"dm"`。

- [ ] **Step 4: 修第三处(line 437-438)**

同上模式。先 Read 一下确认上下文(可能是 `channels_dir` 在 reindex 路径里)。

- [ ] **Step 5: 修第四处(line 469-470)**

同上模式。

- [ ] **Step 6: 验证 clippy 通过**

Run: `cargo clippy -p gitim-index --all-targets --no-deps 2>&1 | tail -5`

Expected: `Finished` 一行,无 error。

- [ ] **Step 7: 验证测试通过**

Run: `cargo test -p gitim-index 2>&1 | tail -10`

Expected: `test result: ok. N passed`,reindex 相关测试不挂(确认行为没变)。

- [ ] **Step 8: Commit**

```bash
git add crates/gitim-index/src/lib.rs
git commit -m "$(cat <<'EOF'
fix(gitim-index): collapse manual_flatten in reindex loops

clippy::manual_flatten was failing under deny(warnings).
Replace `for entry in read_dir(...).flatten() { if let Ok(entry) = entry { ... } }`
with `for entry in read_dir(...).flatten().flatten() { ... }`.
Semantics unchanged: both forms silently drop read_dir errors and
per-entry errors.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: 修 `gitim-client::search` `too_many_arguments`

**Files:**
- Modify: `crates/gitim-client/src/client.rs:327`

加 `#[allow(clippy::too_many_arguments)]` attribute,跟代码库现有 14 处同款处理一致(`crates/gitim-cli/src/commands/admin.rs:63` / `crates/gitim-sync/src/sync_loop.rs:95` 等)。不重构成 `SearchRequest` struct,因为那会引入跟现有模式不一致的孤例。

- [ ] **Step 1: 加 `#[allow]` attribute**

Edit `crates/gitim-client/src/client.rs`,在 line 327 `pub async fn search(` 上一行加:

```rust
    #[allow(clippy::too_many_arguments)]
    pub async fn search(
```

- [ ] **Step 2: 验证 clippy 通过**

Run: `cargo clippy -p gitim-client --all-targets --no-deps 2>&1 | tail -3`

Expected: `Finished` 一行,无 error。

- [ ] **Step 3: Commit**

```bash
git add crates/gitim-client/src/client.rs
git commit -m "$(cat <<'EOF'
fix(gitim-client): allow too_many_arguments on search()

search() takes 7 user params + &self = 8, one over the 7 threshold.
Use #[allow] consistent with the 14 existing call sites in the codebase
(admin.rs, sync_loop.rs, all agent provider modules). A SearchRequest
struct would create a stylistic outlier without improving call sites.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: 验证 workspace.lints 分组隔离假设

**Files:** 无修改 — 这是 empirical 实验。

Spec 风险节最重要的不确定性:`[workspace.lints.rust] warnings = "deny"` 是否会把 clippy 的 warn 级别 lint(如 `unwrap_used`)升级为 deny。需要先验证,如果踩坑就回退方案。

- [ ] **Step 1: 临时加 minimal workspace.lints 到根 Cargo.toml**

Edit `Cargo.toml`,在 `[workspace.dependencies]` 上面加:

```toml
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
unwrap_used = "warn"
```

- [ ] **Step 2: 临时把 gitim-core/Cargo.toml 设为继承**

Edit `crates/gitim-core/Cargo.toml`,末尾加:

```toml
[lints]
workspace = true
```

- [ ] **Step 3: 临时删 gitim-core/src/lib.rs 顶部的 `#![deny(warnings)]`**

Edit `crates/gitim-core/src/lib.rs`,删第 1 行的 `#![deny(warnings)]`。

- [ ] **Step 4: 跑 clippy,看 unwrap 是 warn 还是 error**

Run: `cargo clippy -p gitim-core --no-deps 2>&1 | grep -E "(warning|error).*unwrap_used" | head -3`

**Expected case A(假设成立)**:输出 `warning: used `.unwrap()` ...`,**不是** `error`。继续 Task 4。

**Expected case B(假设不成立)**:输出 `error: used `.unwrap()` ...`,implied by `deny(warnings)`。Stop。回到 spec 调整方案:把 `[workspace.lints.rust] warnings = "deny"` 换成 `[workspace.lints.rust] unused = { level = "deny", priority = -1 }`(只 deny `unused_*` lint,不跨进 clippy)。然后改 Task 4 配置。

- [ ] **Step 5: 撤销实验性改动**

Run:
```bash
git checkout Cargo.toml crates/gitim-core/Cargo.toml crates/gitim-core/src/lib.rs
```

(实验后回退,Task 4 用最终决定的配置一次性写入。)

- [ ] **Step 6: 不 commit,记结论用于 Task 4**

写在 conversation 上下文里:"workspace.lints 分组 [isolated | NOT isolated],Task 4 用 [A 方案 | B 方案]"。

---

## Task 4: 写入 workspace.lints 配置

**Files:**
- Modify: `Cargo.toml`

按 Task 3 验证结果写入。**A 方案(假设成立)**:

```toml
[workspace.lints.rust]
warnings = "deny"

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }

# 纪律 lint(deny — bug / 草稿残留)
dbg_macro = "deny"
print_stdout = "deny"
print_stderr = "deny"
todo = "deny"
unimplemented = "deny"

# 纪律 lint(warn — 测试中合理,生产代码 review;后续逐步 deny)
unwrap_used = "warn"
expect_used = "warn"
panic = "warn"
```

**B 方案(假设不成立)**:把 `warnings = "deny"` 改成:

```toml
[workspace.lints.rust]
unused = { level = "deny", priority = -1 }
deprecated = "deny"
```

- [ ] **Step 1: Edit Cargo.toml,根据 Task 3 结论加 `[workspace.lints.*]` 表**

位置:`[workspace.dependencies]` **上面**(让 lints 紧跟 `[workspace.package]`)。

- [ ] **Step 2: 不 commit,Task 5/6/7 一起 commit**

---

## Task 5: 所有 10 个 crate 加 `[lints] workspace = true`

**Files:**
- Modify: `crates/gitim-core/Cargo.toml`
- Modify: `crates/gitim-daemon/Cargo.toml`
- Modify: `crates/gitim-sync/Cargo.toml`
- Modify: `crates/gitim-index/Cargo.toml`
- Modify: `crates/gitim-client/Cargo.toml`
- Modify: `crates/gitim-cli/Cargo.toml`
- Modify: `crates/gitim-agent-provider/Cargo.toml`
- Modify: `crates/gitim-runtime/Cargo.toml`
- Modify: `crates/gitim-updater/Cargo.toml`
- Modify: `crates/gitim-wasm/Cargo.toml`

每个 crate 的 `Cargo.toml` 末尾加:

```toml
[lints]
workspace = true
```

- [ ] **Step 1: 对每个 crate 加配置**

10 个 Edit 操作。每个 Cargo.toml 末尾追加 `[lints]\nworkspace = true\n` 即可。

- [ ] **Step 2: 验证还能 build**

Run: `cargo build --workspace 2>&1 | tail -3`

Expected: `Finished` 一行,无 error。

- [ ] **Step 3: 不 commit,合并到 Task 7**

---

## Task 6: 删 8 个 crate 顶部的 `#![deny(warnings)]`

**Files:**
- Modify: `crates/gitim-core/src/lib.rs:1`
- Modify: `crates/gitim-client/src/lib.rs:1`
- Modify: `crates/gitim-updater/src/lib.rs:1`
- Modify: `crates/gitim-index/src/lib.rs:1`
- Modify: `crates/gitim-sync/src/lib.rs:1`
- Modify: `crates/gitim-runtime/src/lib.rs:1`
- Modify: `crates/gitim-cli/src/main.rs:1`
- Modify: `crates/gitim-daemon/src/main.rs:1`

每个文件第 1 行是 `#![deny(warnings)]`,删掉(workspace.lints 已覆盖)。

- [ ] **Step 1: 用 grep 二次确认 8 个位置都是第 1 行**

Run: `grep -nH "deny(warnings)" crates/*/src/lib.rs crates/*/src/main.rs`

Expected: 8 行,全是 `:1:#![deny(warnings)]`。

- [ ] **Step 2: 对每个文件 Edit 删第 1 行**

8 个 Edit 操作。删 `#![deny(warnings)]` 行 + 紧跟的空行(如果有)。

- [ ] **Step 3: 不 commit,合并到 Task 7**

---

## Task 7: 验证全 workspace clippy/build/test 通过 + 合并 commit

**Files:** 无新修改,这步是验收 Task 4/5/6 联合产物。

- [ ] **Step 1: cargo fmt**

Run: `cargo fmt --all`

Expected: 无输出(无格式问题)或自动修复后无输出。

- [ ] **Step 2: cargo build --workspace**

Run: `cargo build --workspace 2>&1 | tail -3`

Expected: `Finished` 一行。

- [ ] **Step 3: cargo clippy --workspace --all-targets**

Run: `cargo clippy --workspace --all-targets --no-deps 2>&1 | tail -5`

Expected: `Finished` 一行。允许有 warning(unwrap/expect/panic),**不允许** error。

- [ ] **Step 4: cargo test --workspace(baseline 回归 check)**

Run: `cargo test --workspace 2>&1 | tail -5`

Expected: `test result: ok` 全部 crate(700+ 测试,数分钟级别)。如果有 pre-existing 红测试跟本 PR 无关,记下来不 block。

- [ ] **Step 5: 故意写一行 dbg!() 验证 deny 生效**

临时在 `crates/gitim-core/src/lib.rs` 末尾追加:

```rust
#[allow(dead_code)]
fn _hook_smoke_test() {
    let x = 1;
    dbg!(x);
}
```

Run: `cargo clippy -p gitim-core --no-deps 2>&1 | grep -E "(error|warning).*dbg_macro" | head -2`

Expected: `error: ... clippy::dbg_macro`(`error` 而不是 `warning`,证明 deny 生效)。

撤销改动:`git checkout crates/gitim-core/src/lib.rs`。

- [ ] **Step 6: Commit Task 4/5/6**

```bash
git add Cargo.toml crates/*/Cargo.toml crates/*/src/lib.rs crates/*/src/main.rs
git commit -m "$(cat <<'EOF'
chore: workspace clippy lints + per-crate inherit

Move per-crate #![deny(warnings)] discipline to a single
[workspace.lints] config:
- rust: warnings = deny (or fallback: unused/deprecated = deny)
- clippy::all = deny (correctness/suspicious/complexity/perf/style)
- dbg_macro / print_stdout / print_stderr / todo / unimplemented = deny
- unwrap_used / expect_used / panic = warn (surface for future cleanup)

Each crate's Cargo.toml gains [lints] workspace = true. Removed the
now-redundant #![deny(warnings)] from 8 crate roots. gitim-agent-provider
and gitim-wasm gain coverage they previously lacked.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: 写 pre-commit hook 源文件

**Files:**
- Create: `scripts/hooks/pre-commit`

- [ ] **Step 1: 创建目录**

Run: `mkdir -p scripts/hooks`

- [ ] **Step 2: 写 hook 文件**

Create `scripts/hooks/pre-commit`:

```bash
#!/usr/bin/env bash
# Pre-commit hook: enforce cargo fmt + cargo clippy on the whole workspace.
# Install via: scripts/install-hooks.sh
# Bypass (emergency only): git commit --no-verify
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

echo "==> cargo fmt --all -- --check"
if ! cargo fmt --all -- --check; then
  echo
  echo "ERROR: cargo fmt found unformatted code. Run 'cargo fmt --all' and re-stage."
  exit 1
fi

echo "==> cargo clippy --workspace --all-targets --no-deps --locked"
if ! cargo clippy --workspace --all-targets --no-deps --locked; then
  echo
  echo "ERROR: cargo clippy failed. Fix the errors above (or use --no-verify in emergencies)."
  exit 1
fi
```

- [ ] **Step 3: chmod +x**

Run: `chmod +x scripts/hooks/pre-commit`

- [ ] **Step 4: 不 commit,合并到 Task 10**

---

## Task 9: 写安装器

**Files:**
- Create: `scripts/install-hooks.sh`

- [ ] **Step 1: 写安装器**

Create `scripts/install-hooks.sh`:

```bash
#!/usr/bin/env bash
# Install GitIM repo git hooks into the shared git-common-dir (worktree-safe).
# Idempotent: re-run anytime to refresh. ln -sf means source updates flow through.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOKS_DIR="$(git -C "$ROOT" rev-parse --git-common-dir)/hooks"

if [ ! -d "$HOOKS_DIR" ]; then
  mkdir -p "$HOOKS_DIR"
fi

SRC="$ROOT/scripts/hooks/pre-commit"
if [ ! -x "$SRC" ]; then
  echo "ERROR: $SRC not found or not executable."
  exit 1
fi

DST="$HOOKS_DIR/pre-commit"
ln -sf "$SRC" "$DST"

echo "Installed: $DST -> $SRC"
echo "Bypass an emergency commit with: git commit --no-verify"
```

- [ ] **Step 2: chmod +x**

Run: `chmod +x scripts/install-hooks.sh`

- [ ] **Step 3: 不 commit,合并到 Task 10**

---

## Task 10: 装 hook + 端到端验证

**Files:** 无修改。

- [ ] **Step 1: 装 hook**

Run: `scripts/install-hooks.sh`

Expected: `Installed: /Users/lewisliu/ateam/GitIM/.git/hooks/pre-commit -> /Users/lewisliu/ateam/GitIM/.claude/worktrees/clever-wright-087e11/scripts/hooks/pre-commit`(注意 git-common-dir 是 main 仓库的 .git,不是 worktree 的 .git 文件)。

- [ ] **Step 2: 验证 symlink 落对位置**

Run: `ls -la "$(git rev-parse --git-common-dir)/hooks/pre-commit"`

Expected: 显示一个 symlink,target 是 worktree 下的源文件。

- [ ] **Step 3: 正向验证 — clean commit 通过**

Run:
```bash
echo "" >> /tmp/dummy-test.md   # 不放仓库内
git status --short    # 应该没未跟踪改动
git commit --allow-empty -m "test: hook accepts clean state"
```

Expected: hook 跑 fmt + clippy,通过,commit 创建成功。**事后 `git reset HEAD~1` 撤掉这个 empty commit**。

- [ ] **Step 4: 负向验证 — fmt 坏的拒绝**

临时改一个 .rs 文件加 2 个空格 indent 错位 → `git add` → `git commit`。

Run:
```bash
# 找一个无关紧要的 .rs 文件,加坏 indent
echo "fn _hook_test() {  let x = 1;}" >> crates/gitim-core/src/lib.rs
git add crates/gitim-core/src/lib.rs
git commit -m "test: hook should reject"
```

Expected: hook 报 `ERROR: cargo fmt found unformatted code.`,exit non-zero,commit 不创建。

撤销:`git checkout crates/gitim-core/src/lib.rs`。

- [ ] **Step 5: 负向验证 — clippy error 拒绝**

临时在 `crates/gitim-core/src/lib.rs` 末尾追加(同 Task 7 Step 5):

```rust
#[allow(dead_code)]
fn _hook_smoke_test() {
    let x = 1;
    dbg!(x);
}
```

Run:
```bash
git add crates/gitim-core/src/lib.rs
git commit -m "test: hook should reject"
```

Expected: hook 输出 `==> cargo clippy ...`,跑到 dbg_macro 行报 `error: ... clippy::dbg_macro`,最后输出 `ERROR: cargo clippy failed.`,exit non-zero,commit 不创建(`git log -1` 还是上一个 commit)。

撤销:
```bash
git reset HEAD crates/gitim-core/src/lib.rs
git checkout crates/gitim-core/src/lib.rs
```

- [ ] **Step 6: 不 commit,合并到 Task 11**

---

## Task 11: 更新 CLAUDE.md + commit hook 基础设施

**Files:**
- Modify: `CLAUDE.md`

在 `## Rust toolchain policy` 段后面或某个合适位置加新段落 `## Pre-commit hook`。

- [ ] **Step 1: 看现有 CLAUDE.md 找合适插入点**

Run: `grep -n "^## " CLAUDE.md`

挑 `## 测试` 之前是合适位置(pre-commit 跟 dev workflow 相关)。

- [ ] **Step 2: Edit CLAUDE.md 加段落**

新增段落(放在 `## 测试` 上面):

```markdown
## Pre-commit hook

新 clone 后跑一次:

```bash
scripts/install-hooks.sh
```

会把 `scripts/hooks/pre-commit` symlink 到 git-common-dir 的 hooks 目录(worktree-safe,一次装所有 worktree 共享)。每次 `git commit` 自动跑:

1. `cargo fmt --all -- --check` — 不通过则拒绝 commit
2. `cargo clippy --workspace --all-targets --no-deps --locked` — error 则拒绝 commit

冷启 30s+,sccache 增量秒级。

紧急逃生(慎用):`git commit --no-verify`。

Lint 规则在根 `Cargo.toml` 的 `[workspace.lints]` 单点维护(`clippy::all = deny`、`dbg_macro` / `print_stdout` / `todo` / `unimplemented` = deny、`unwrap_used` / `expect_used` / `panic` = warn)。新 crate 加进 workspace 时,在它的 `Cargo.toml` 末尾加 `[lints]\nworkspace = true` 继承。
```

- [ ] **Step 3: Commit Task 8/9/10/11 一起**

```bash
git add scripts/hooks/pre-commit scripts/install-hooks.sh CLAUDE.md
git commit -m "$(cat <<'EOF'
chore: pre-commit hook enforcing cargo fmt + cargo clippy

scripts/hooks/pre-commit is the versioned source; scripts/install-hooks.sh
symlinks it into git rev-parse --git-common-dir/hooks (worktree-safe,
install once for all worktrees). Hook fails on:
- cargo fmt --all -- --check
- cargo clippy --workspace --all-targets --no-deps --locked

unwrap/expect/panic are warn-level so they surface without blocking commits.
Bypass with git commit --no-verify in emergencies.

CLAUDE.md updated with the install step + brief workspace.lints note for
adding new crates.

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 12: 最终全量回归

**Files:** 无修改。

- [ ] **Step 1: 全量 cargo build**

Run: `cargo build --workspace 2>&1 | tail -3`

Expected: `Finished`。

- [ ] **Step 2: 全量 cargo clippy**

Run: `cargo clippy --workspace --all-targets --no-deps 2>&1 | tail -5`

Expected: `Finished`,无 error。可能有 unwrap/expect/panic warning,正常。

- [ ] **Step 3: 全量 cargo test**

Run: `cargo test --workspace 2>&1 | tail -10`

Expected: `test result: ok` 全部 crate。比对 baseline,本 PR 不应引入新 failure。

- [ ] **Step 4: git log 看 commits 数和顺序**

Run: `git log --oneline main..HEAD`

Expected: 4 个新 commit,顺序:
1. `docs: workspace clippy lints + pre-commit hook design`(已 commit)
2. `fix(gitim-index): collapse manual_flatten in reindex loops`(Task 1)
3. `fix(gitim-client): allow too_many_arguments on search()`(Task 2)
4. `chore: workspace clippy lints + per-crate inherit`(Task 7)
5. `chore: pre-commit hook enforcing cargo fmt + cargo clippy`(Task 11)

实际 5 个,加上 design doc commit。

- [ ] **Step 5: 准备进入 finishing-a-development-branch**

汇报给 user,等指示 merge / PR。

---

## Notes for the executor

- 全量 `cargo test` 是分钟级别贵操作,memory 明确 baseline / 任务末尾各跑一次,中间只跑相关 crate(`cargo test -p <crate>`)。
- 装上 hook 后,**本 PR 后续每个 commit 都会被 hook 检查**。Task 11 commit 是第一个走 hook 的 commit,如果它通过就证明 hook 工作。
- Memory 提示:`feedback_cargo_fmt_before_commit.md` 已是惯例,hook 把它正式化。
- `priority = -1` 给 `clippy::all` 是为了让具体 lint(如 `dbg_macro`)的 deny 不被 group level 覆盖 — 这是 workspace.lints 的标准用法。
- 如果 Task 3 实验显示分组隔离不成立(B 方案),Task 7 commit message 描述要相应改成 `unused/deprecated = deny` 而不是 `warnings = deny`。
