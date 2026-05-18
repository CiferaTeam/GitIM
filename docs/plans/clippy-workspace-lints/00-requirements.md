# Workspace Clippy Lints + Pre-commit Hook

**Date**: 2026-05-18
**Status**: Design approved, ready for implementation plan

## 问题

工程现状:

- 10 个 crate 的 workspace,根 `Cargo.toml` 没有 `[workspace.lints]` 配置。
- 8 个 crate 在 `lib.rs` / `main.rs` 顶部写了 `#![deny(warnings)]`(`gitim-core` / `gitim-client` / `gitim-updater` / `gitim-index` / `gitim-sync` / `gitim-runtime` / `gitim-cli` / `gitim-daemon`),另 2 个没写(`gitim-agent-provider` / `gitim-wasm`)。规则散落在每个 crate,新增 crate 容易漏。
- 没有 git hook 强制 lint。`scripts/lint-shell.sh` 是已有的 lint 脚本范例(shellcheck),但只手动跑。
- CI 只有 `.github/workflows/release.yml`,没有 PR / branch lint gate。
- 当前实际 5 个 clippy error 长期存在没人发现(4 个 `manual_flatten` in `gitim-index`,1 个 `too_many_arguments` in `gitim-client::client::search`),因为这些 crate 虽然写了 `#![deny(warnings)]` 但没人主动跑 `cargo clippy`。

目标:

1. 把 lint 纪律从"每个 crate 各自写 attribute"上移到 workspace 单一配置点。
2. 提交前自动跑 `cargo fmt --check` 和 `cargo clippy --workspace`,失败拒绝 commit。
3. 现存 5 个 clippy error 作为 prerequisite 在同一个 PR 修掉(否则 hook 装上立刻全员 commit 不了)。

## 非目标(v1 不做)

- **CI lint workflow**(`.github/workflows/lint.yml`):hook 可被 `--no-verify` 绕过,CI 是后续值得加的第二道防线。本次 scope 外。
- **`clippy::pedantic` / `clippy::nursery`**:开启会增加 ~196 个 warning(已用 `cargo clippy -- -W clippy::pedantic` 实测),大量是 `must_use_candidate` / `module_name_repetitions` 这类 stylistic noise。后续可单独 triage 开启。
- **`cargo-husky` 自动装 hook**:加 dev-dependency,引入隐式行为,跟现有 `scripts/lint-shell.sh` 的显式风格不一致。手动装一次性成本可接受。
- **批量清理 unwrap/expect/panic**:`crates/*/src/` 下 1173 处(混含 inline `mod tests`),`tests/` 下 2366 处。一次性清理会让 PR 失去聚焦。本次 PR 把 lint 设为 `warn` 让它们暴露,清理是后续逐 crate 的工作。

## 设计

### A. Workspace lint 配置

根 `Cargo.toml` 增加:

```toml
[workspace.lints.rust]
warnings = "deny"        # 替代各 crate 的 #![deny(warnings)]

[workspace.lints.clippy]
all = { level = "deny", priority = -1 }   # correctness + suspicious + complexity + perf + style

# 纪律 lint(deny — 这些是 bug / 草稿残留,不是 style 偏好)
dbg_macro = "deny"
print_stdout = "deny"    # 项目用 tracing
print_stderr = "deny"
todo = "deny"
unimplemented = "deny"

# 纪律 lint(warn — 测试中合理用法,但生产代码该被 review;后续逐步迁向 deny)
unwrap_used = "warn"
expect_used = "warn"
panic = "warn"
```

每个 crate 的 `Cargo.toml` 末尾加:

```toml
[lints]
workspace = true
```

每个 crate 的 `lib.rs` / `main.rs` 删除 `#![deny(warnings)]`(workspace 已覆盖)。

**关键语义**:`[workspace.lints.rust] warnings = "deny"` 只 deny rust 注册的 warning lint(`unused_imports` / `dead_code` / `deprecated` 等),不跨进 clippy 组。clippy 组的 deny / warn 由 `[workspace.lints.clippy]` 单独控制。这跟 inline `#![deny(warnings)]` 跨组升级的行为不同 — inline attribute 在 lib.rs 顶部会把同 crate 内所有 clippy warn 升级为 deny,而 workspace lints 表分组隔离。所以 `unwrap_used = "warn"` 在新配置下不会被 `warnings = "deny"` 顺手 escalate(实现阶段需要 empirical verify;若 Cargo workspace lints 实际行为有 surprise,fallback 是不设 `[workspace.lints.rust] warnings = "deny"`,改用 `unused = { level = "deny", priority = -1 }` 覆盖最常见的 rust warn lint)。

### B. 先修 5 个现存 clippy error

prerequisite:hook 装上之前必须修完,否则现有 commit 路径直接锁死。

- **`crates/gitim-index/src/lib.rs`**:4 个 `manual_flatten`(`for entry in ...read_dir(...).into_iter().flatten() { if let Ok(entry) = entry { ... } }`)。改成 `for entry in ...read_dir(...).into_iter().flatten().flatten()` 或重写循环结构。
- **`crates/gitim-client/src/client.rs:327`** `search()`:8 个参数(`query` / `author` / `channel` / `limit` / `offset` / `include_channels` / `include_dms` / `include_cards`)超 7 阈值。两种修法:
  - **首选**:抽 `SearchRequest` struct 把参数打包(顺带让 call site 更可读)。
  - **次选**:`#[allow(clippy::too_many_arguments)]` 加 `// SAFETY: search 入参语义独立,struct 化不增可读性`(实现阶段评估改造成本后决定)。

### C. Pre-commit hook

**源文件**(版本化):`scripts/hooks/pre-commit`

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

echo "==> cargo fmt --check"
cargo fmt --all -- --check

echo "==> cargo clippy"
cargo clippy --workspace --all-targets --no-deps --locked
```

不跑 `-D warnings` flag — 让 workspace 配置自己决定 deny vs warn,unwrap_used 这种 warn 级别才能保留信号而不阻断 commit。

**安装器**:`scripts/install-hooks.sh`

```bash
#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
HOOKS_DIR="$(git -C "$ROOT" rev-parse --git-common-dir)/hooks"  # worktree-safe
SRC="$ROOT/scripts/hooks/pre-commit"

mkdir -p "$HOOKS_DIR"
ln -sf "$SRC" "$HOOKS_DIR/pre-commit"
echo "Installed: $HOOKS_DIR/pre-commit -> $SRC"
```

`ln -sf` 而不是 `cp`:源文件改动自动生效,不需要再装一次。worktree 共享同一个 hooks dir,装一次所有 worktree 都用上。

**逃生口**:`git commit --no-verify` 是标准 git 行为,不另造旁路开关。

### D. 文档变更

- `CLAUDE.md` 加一段:新 clone 后跑 `scripts/install-hooks.sh` 装 pre-commit hook。
- 提及:`cargo clippy --workspace` 在 sccache + 增量下秒级,冷启 30s+。

## 测试 / 验证

实现阶段需要 empirical 验证:

1. workspace lints 配置 + 删除 per-crate `#![deny(warnings)]` 后,`cargo build --workspace` 能过。
2. `cargo clippy --workspace --all-targets --no-deps` 退出 0(警告允许,错误不许)。
3. 故意写一行 `dbg!(x)` / `println!(...)` / `unimplemented!()`,clippy 失败。
4. 故意写一行 `x.unwrap()`,clippy 退出 0 但输出 warning。
5. `scripts/install-hooks.sh` 在主 worktree 和 clever-wright-087e11 worktree 都成功(共享同一个 hooks dir)。
6. 装上 hook 后,`git commit -m test` 在 fmt-bad / clippy-bad / clean 三种状态下分别拒绝 / 拒绝 / 通过。

## 速度预期

- Hook 冷启:30s+(workspace 全量 clippy)。
- 有 sccache 跨 worktree 复用 + 增量缓存:秒级。
- 不跑 `cargo test`(memory 已明确:全量 test 是分钟级别贵操作,只在 task 末尾跑)。

## 风险

- **`workspace.lints` 分组隔离假设错了**:即 `[workspace.lints.rust] warnings = "deny"` 实际还是把 clippy warn 升级为 deny,则 `unwrap_used = "warn"` 立刻变 1173+ 个 error,blocker。实现阶段第一步就 empirical 测,真踩坑就 fallback 到 `unused = "deny"` 覆盖最常见 rust lint(`unused_imports` / `dead_code` / `unused_variables`)。
- **5 个现存 error 修法争议**:`too_many_arguments` 改 `SearchRequest` struct 会动 call site,影响面要看 client 调用点。实现阶段先 grep 出 call site 决定改还是 allow。
- **新 contributor 忘装 hook**:CLAUDE.md 提示 + 后续可加 CI lint workflow 兜底(v2)。
