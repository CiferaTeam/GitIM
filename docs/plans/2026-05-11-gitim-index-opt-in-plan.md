# gitim-index Opt-in 化 — 实施计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec**: [2026-05-11-gitim-index-opt-in.md](2026-05-11-gitim-index-opt-in.md)

**Goal**: 为 gitim-index 加 per-clone `indexer.enabled` 开关（默认 false），让 agent daemon 不再后台维护无人使用的 FTS5 索引，human clone 保留搜索能力（onboard 路径显式 opt-in）。

**Architecture**: 在 `gitim_core::types::config::Config` 加 `IndexerConfig` 子结构；daemon 启动时分支决定是否 `initialize_index`；现有 `state.index: Option<Arc<Index>>` 的 None 路径已经在 sync_loop / search handler / reindex handler 里被正确处理，只需要给它一个合法触发条件。CLI `gitim onboard` 与 Runtime `provision_human` 两条 human 路径在写完 `me.json` 后 patch config.yaml 为 `enabled=true`。Agent 路径零改动，依赖 daemon 启动时写 default config 的现有逻辑。

**Tech Stack**: Rust, serde (yaml), tokio, rusqlite (不动)。

**约束**:
- 每 task 内最后一步必须 commit（项目 feedback：commit per task）
- 测试节奏：先写失败测试 → 跑 → 失败 → 改实现 → 跑 → 通过
- 中间 task 只跑相关 crate / 相关 `--test` 目标，不跑全量；Task 6 才全量回归（项目 feedback：cargo test 全量贵，不要无脑跑）

---

## Task 1: 核心 `IndexerConfig` 数据结构

**Files**:
- Modify: `crates/gitim-core/src/types/config.rs`
- Test: 同文件 `#[cfg(test)] mod tests`（与现有 `config_default_*` 测试并列）

**目标**: 在 `Config` 增加 `indexer: IndexerConfig` 子字段，子结构含单字段 `enabled: bool`，默认 `false`。旧 yaml 不带 `indexer:` 字段也能反序列化。

- [ ] **Step 1**: 在 `crates/gitim-core/src/types/config.rs` 的 `#[cfg(test)] mod tests` 里新增三个失败测试：
  - `indexer_defaults_to_disabled` —— 断言 `Config::default().indexer.enabled == false`
  - `legacy_yaml_without_indexer_field_parses` —— 用一段不含 `indexer:` 字段的 yaml 字符串反序列化为 `Config`，断言反序列化成功且 `indexer.enabled == false`
  - `config_default_roundtrips_with_indexer` —— 仿现有 `config_default_roundtrips_through_yaml`，但额外断言 yaml 文本里包含 `indexer:` 段且 roundtrip 后 `enabled` 值保持

- [ ] **Step 2**: 跑 `cargo test -p gitim-core types::config` 确认三个新测试因 `IndexerConfig` 未定义而编译失败

- [ ] **Step 3**: 在 `config.rs` 加 `IndexerConfig` 子结构定义：派生 `Debug, Clone, Serialize, Deserialize, PartialEq, Default`；单字段 `pub enabled: bool`（`bool::default()` 即 false，无需显式 `#[serde(default)]` 在字段上）

- [ ] **Step 4**: 在 `Config` 加 `#[serde(default)] pub indexer: IndexerConfig`，并在 `impl Default for Config` 里加 `indexer: IndexerConfig::default()` 字段

- [ ] **Step 5**: 跑 `cargo test -p gitim-core types::config` 确认三个新测试 + 现有所有 `config_*` 测试全部通过

- [ ] **Step 6**: 跑 `cargo test -p gitim-core` 整 crate 通过（确认 validator / 其他模块没被 Config schema 变化影响）

- [ ] **Step 7**: Commit。
  - 标题：`feat(core): add IndexerConfig sub-struct, default disabled`
  - body 简述：新增 per-clone 开关数据结构；旧 yaml 兼容（serde default false）；不影响其他字段。

---

## Task 2: Daemon 启动分支 + None 路径生效

**Files**:
- Modify: `crates/gitim-daemon/src/state.rs`（`initialize_index` 函数，行 ~147）
- Modify: `crates/gitim-daemon/src/main.rs`（调用 `initialize_index` 的位置）
- Test: 新建 `crates/gitim-daemon/tests/indexer_disabled_test.rs`

**目标**: daemon 启动时如果 `config.indexer.enabled == false`，跳过 `initialize_index` 内部所有 SQLite / git diff 操作，`state.index` 保持 `None`。sync_loop 已有 None 分支，零改动即生效。

**前置阅读**: design doc "Daemon 启动分支" 节、`state.rs:313-358` sync_loop hook（确认 None 已被处理）。

- [ ] **Step 1**: 看 `crates/gitim-daemon/tests/` 下任一现有集成测试（例如 `handlers_test.rs`）了解 daemon 启动 / 临时仓库 / IPC 客户端的测试 fixture 模式

- [ ] **Step 2**: 新建 `crates/gitim-daemon/tests/indexer_disabled_test.rs`，写一个失败集成测试 `indexer_disabled_skips_index_creation`：
  - 在 tempdir 初始化 git repo + `.gitim/config.yaml`（含 `indexer.enabled: false`）+ 最少必要的 `users/<handler>.meta.yaml` 与 `me.json`
  - 启动 daemon，等启动完成（仿现有测试的 readiness 等待方式）
  - 触发一次同步 cycle 或等到至少一次 sync tick 后
  - 断言：`<repo>/.gitim/index.db` 文件**不存在**
  - 断言：通过 IPC 发送 `Request::Search { query: Some("anything"), .. }` 返回 `Response::error`（**只断言"是 error"，不断言错误文案** —— 文案在 Task 3 敲定，避免两 task 耦合）

- [ ] **Step 3**: 跑 `cargo test -p gitim-daemon --test indexer_disabled_test` 确认测试因当前 daemon 仍创建 `index.db` 而失败

- [ ] **Step 4**: 修改 `state::initialize_index`：函数签名加 `enabled: bool` 参数（或读取 `state` 上已存的 config —— 选择更自然的方式）。在函数开头，如果 `enabled == false`，记录 `tracing::info!("indexer disabled by config")`，直接 `return Ok(())`，**不**打开 SQLite、**不**写 `state.index`

- [ ] **Step 5**: 修改 `crates/gitim-daemon/src/main.rs` 中调 `initialize_index` 的地方，传入 `config.indexer.enabled`

- [ ] **Step 6**: 跑 `cargo test -p gitim-daemon --test indexer_disabled_test` 确认两条断言均通过

- [ ] **Step 7**: **关键审计步骤** —— 跑 `cargo test -p gitim-daemon` 全 daemon 测试。预计有部分现有测试会失败：它们假设 daemon 启动后 search 可用，但新 default config（如果由 daemon 自创建）的 `indexer.enabled` 是 false。识别这些测试并在它们的 fixture 中显式写 `indexer.enabled: true` 到 config.yaml（在 spawn daemon 之前）。
  - 失败定位：搜索使用 `Request::Search` 或 `handle_search` 的测试，或者依赖 `index.db` 存在的测试
  - 修复模式：测试 setup 阶段写 config.yaml 时显式设 `indexer.enabled: true`

- [ ] **Step 8**: 再次跑 `cargo test -p gitim-daemon` 确认全绿

- [ ] **Step 9**: Commit。
  - 标题：`feat(daemon): honor IndexerConfig.enabled at startup`
  - body：默认 false 时跳过 initialize_index，state.index 保持 None；sync_loop / handlers 复用现有 None 分支。

---

## Task 3: Handler 错误文案改进

**Files**:
- Modify: `crates/gitim-daemon/src/handlers/search.rs`（`handle_search` 第 ~19 行、`handle_reindex` 第 ~70 行各一处 `Response::error(...)`）
- Test: 复用 Task 2 的 `indexer_disabled_test.rs`，新增一条断言或修改 Task 2 中已写的 "disabled" 断言

**目标**: 把 "search index not available" 改为 "search index disabled for this clone (set indexer.enabled=true in .gitim/config.yaml and restart daemon)"，让 CLI 用户在 agent clone 上跑 `gitim search` 时得到可操作信息。

- [ ] **Step 1**: 在 `indexer_disabled_test.rs` 加（或确认已存在）一条 `assert!(error_string.contains("disabled"))` 针对 search，并新增一条针对 reindex 的同模式断言（发 `Request::Reindex` 看返回 error 含 "disabled"）。如果 Task 2 已写了，确认两个都在。

- [ ] **Step 2**: 跑 `cargo test -p gitim-daemon --test indexer_disabled_test` 确认 reindex 那条（或两条）因当前文案为 "not available" 而失败

- [ ] **Step 3**: 改 `handlers/search.rs` 中 `handle_search` 的 `Response::error("search index not available")` 为 `Response::error("search index disabled for this clone (set indexer.enabled=true in .gitim/config.yaml and restart daemon)")`

- [ ] **Step 4**: 改 `handle_reindex` 中同样的文案（第 ~70 行）为同一字符串

- [ ] **Step 5**: 跑 `cargo test -p gitim-daemon --test indexer_disabled_test` 确认两条断言通过

- [ ] **Step 6**: 跑 `cargo test -p gitim-daemon` 全 daemon 测试通过（确认没有其他测试断言旧文案）

- [ ] **Step 7**: Commit。
  - 标题：`feat(daemon): clarify search/reindex error when indexer disabled`
  - body：错误信息从 "not available" 改为 "disabled" + 指引 config 路径。

---

## Task 4: CLI `gitim onboard` 显式写 `indexer.enabled=true`

**Files**:
- Modify: `crates/gitim-cli/src/commands/onboard.rs`（仿 `ensure_config_debug_http`，约 352 行）
- Test: `crates/gitim-cli/tests/` 下找现有 onboard 测试位置；若无则新建 `crates/gitim-cli/tests/onboard_indexer_test.rs`

**目标**: CLI 的 `gitim onboard` 流程完成后，`<repo>/.gitim/config.yaml` 必须含 `indexer.enabled: true`，使 human 用户的 daemon 在升级后保持搜索能力。

**前置阅读**: `onboard.rs:352` 处的 `ensure_config_debug_http` 是函数模板（regex 替换 + 新文件创建两种情况都处理了），仿写即可。

- [ ] **Step 1**: 看 `crates/gitim-cli/tests/` 现有结构，找一个 onboard 测试参考（如不存在再决定新建测试文件）

- [ ] **Step 2**: 写失败测试 `onboard_writes_indexer_enabled_true`：
  - 在 tempdir 跑 `cmd_onboard` 或等价代码路径
  - 完成后读取 `<repo>/.gitim/config.yaml`
  - 反序列化为 `gitim_core::types::config::Config`
  - 断言 `config.indexer.enabled == true`

- [ ] **Step 3**: 跑 `cargo test -p gitim-cli onboard_writes_indexer_enabled_true` 确认失败（当前 onboard 不写 indexer 字段，默认 false）

- [ ] **Step 4**: 在 `onboard.rs` 仿 `ensure_config_debug_http` 写 `ensure_config_indexer_enabled(repo_dir: &Path, enabled: bool)`：处理三种情况 —— config 不存在则新建带 `indexer:` 段的最小 yaml；config 存在含 `indexer:` 则 regex 替换；config 存在不含 `indexer:` 则追加段

- [ ] **Step 5**: 在 onboard 主流程合适位置（参考 `ensure_config_debug_http` 的调用点）调 `ensure_config_indexer_enabled(repo_dir, true)`

- [ ] **Step 6**: 跑 `cargo test -p gitim-cli onboard_writes_indexer_enabled_true` 确认通过

- [ ] **Step 7**: 跑 `cargo test -p gitim-cli` 全 CLI 测试通过

- [ ] **Step 8**: Commit。
  - 标题：`feat(cli): write indexer.enabled=true on human onboard`
  - body：CLI onboard 显式开启 indexer，使 human 用户升级后保留 `gitim search` 能力。

---

## Task 5: Runtime `provision_human` 显式写 `indexer.enabled=true`

**Files**:
- Modify: `crates/gitim-runtime/src/agent.rs`（`provision_human`，约 51 行）
- Test: `crates/gitim-runtime/tests/git_init_local.rs` 或 `git_init_*` 系列里新增

**目标**: Runtime `/git/init`（无论 local 还是 github provider）完成 human clone 创建后，human clone 的 `.gitim/config.yaml` 必须含 `indexer.enabled: true`。Agent 路径（`provision_agent`）零改动。

**共享位置**: `ensure_config_indexer_enabled` 函数在 Task 4 写到 `gitim-cli`。Task 5 需要 runtime 也用同一个 —— 把它提到 `gitim-core` 一个新模块 `gitim_core::config_patch`（pub fn），cli + runtime 都 import。理由：cli 已 depends gitim-core；runtime 通过 client / agent-provider 间接依赖 core，必要时在 `crates/gitim-runtime/Cargo.toml` 直接补 `gitim-core = { path = "../gitim-core" }` 即可。core 的 `responses.rs` 等模块虽以纯数据为主，但 `config_patch` 作为 yaml 工具与 `types::config::Config` 同位置语义自洽。

- [ ] **Step 1**: 把 `ensure_config_indexer_enabled` 从 `crates/gitim-cli/src/commands/onboard.rs` 提到 `crates/gitim-core/src/config_patch.rs`（新模块，需在 `lib.rs` `pub mod config_patch;`）。函数签名保持 `(repo_dir: &Path, enabled: bool)`。cli 改成 `use gitim_core::config_patch::ensure_config_indexer_enabled;`。跑 `cargo test -p gitim-cli` + `cargo test -p gitim-core` 确认重构无 regression。Commit：`refactor(core): move ensure_config_indexer_enabled to shared config_patch module`。

- [ ] **Step 2**: 看 `crates/gitim-runtime/tests/git_init_local.rs` 了解 `/git/init` 集成测试模式

- [ ] **Step 3**: 在 `git_init_local.rs` 或新建 `git_init_indexer_test.rs` 写失败测试 `git_init_local_writes_indexer_enabled_true`：
  - 跑 `/git/init` local provider，提供必要 workspace 参数
  - 完成后读 `<workspace>/.gitim-runtime/human/.gitim/config.yaml`
  - 反序列化断言 `indexer.enabled == true`

- [ ] **Step 4**: 跑该测试确认失败

- [ ] **Step 5**: 在 `crates/gitim-runtime/src/agent.rs::provision_human` 函数末尾、`me.json` 写完之后，调用 `ensure_config_indexer_enabled(human_clone_dir, true)`（来自 Step 1 决策的位置）

- [ ] **Step 6**: 跑测试确认通过

- [ ] **Step 7**: 写第二个测试 `add_agent_does_not_enable_indexer`：
  - 跑 `provision_agent` 或等价代码路径
  - agent clone config.yaml 反序列化后断言 `indexer.enabled == false`（即 daemon 自创建 default 时的值）
  - 这是回归保护：防止未来有人误把 ensure 函数也加到 agent 路径

- [ ] **Step 8**: 跑两个测试 + `cargo test -p gitim-runtime` 全 runtime 测试通过

- [ ] **Step 9**: Commit（如果 Step 1 已 commit 重构，则这里是第二个 commit）。
  - 标题：`feat(runtime): write indexer.enabled=true on human /git/init`
  - body：human onboard 路径显式启用 indexer；agent 路径零改动验证测试同步加入。

---

## Task 6: 全量回归 + BREAKING note

**Files**:
- 无代码改动
- 验证：全 workspace 测试

**目标**: 确认无跨 crate regression；用清晰的 commit message 记录 breaking change（项目无 CHANGELOG.md，依赖 git history 作为 release note）。

- [ ] **Step 1**: 跑 `cargo test` 全 workspace 测试。预计接近 700+ 测试，数分钟。**这是 task 末尾全量回归点**，符合项目 feedback：开头一次、末尾一次。

- [ ] **Step 2**: 如有失败：定位失败测试是否为"setup 没显式开 indexer 导致 search 不可用"类型（最可能），按 Task 2 Step 7 的修复模式补 setup；不是这一类的认真分析

- [ ] **Step 3**: 全绿后，更新 spec doc 顶部加一行 `**Status**: implemented in <commit-range>` 或类似（实施完成标记），方便未来回看

- [ ] **Step 4**: 最终 commit。
  - 标题：`docs(plans): mark gitim-index opt-in as implemented`
  - body：包含 BREAKING NOTE 段落，说明：
    - 升级后 daemon 默认不再维护 FTS5 索引
    - Agent clone 行为变化：sync cycle 不再产生 `.gitim/index.db` 增量更新
    - Human clone 升级后若手动想保留 `gitim search`，需在 `.gitim/config.yaml` 显式设 `indexer.enabled: true` 并重启 daemon（新 onboard 自动设）
    - 旧 `.gitim/index.db` 文件保留不动；可手动删除回收空间

- [ ] **Step 5**: 同时更新 CLAUDE.md `Current Orientation` 章节末尾追加一行简述（"gitim-index 改为 opt-in，per-clone 配置 indexer.enabled，agent 默认 off 停掉后台索引"）—— 这是项目 orientation 同步惯例

- [ ] **Step 6**: 二次 commit（如 Step 5 单独提）：`docs(claude.md): record gitim-index opt-in in current orientation`

---

## Self-Review notes（plan 作者笔记，实施时可忽略）

- 所有 task 文件路径已对齐 actual code（config.rs / state.rs:147 / handlers/search.rs:19,70 / onboard.rs:352 / agent.rs:51）
- 没有 placeholder / TBD / "类似 Task N"；每个 step 都有具体动作 + 验证
- Task 2 与 Task 3 的"disabled"断言有依赖：Task 2 引入第一条断言时引用 "not available"，Task 3 把文案切到 "disabled" 并修正断言。如顺序倒过来 / 合并执行，需在合并点协调
- Task 5 的 `ensure_config_indexer_enabled` 共享 (a) vs 内联 (b) 选择由实施者敲定，推荐 (a)
- 全量 `cargo test` 只在 Task 6 跑（项目 feedback：贵，不要无脑跑）
