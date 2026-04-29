# Hermes Profile Isolation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让每个 gitim agent 自动获得一个独立的 hermes profile 目录(`~/.hermes/profiles/gitim-<handler>/`),通过注入 `HERMES_HOME` env 实现 LLM 配置、auth state、session DB 完全隔离,user 在 WebUI 加 agent 时零额外步骤。

**Architecture:**
- 每个 gitim agent 1:1 对应一个 hermes profile,profile 名固定为 `gitim-<handler>`
- 通过子进程注入 `HERMES_HOME=<profile_dir>` 实现隔离,不修改 hermes/mod.rs 命令行
- provision 时 shell out `hermes profile create gitim-<handler> --clone --no-alias`,从 user 的 active profile(默认 `~/.hermes`)拷 `config.yaml` + `.env` + `SOUL.md` + `memories/`
- hard delete 时 shell out `hermes profile delete gitim-<handler> -y`(best-effort)
- preflight 升级以接受 `HERMES_HOME` 参数,确保"该 profile 真能 handshake" ≠ "default profile 能 handshake"

**Tech Stack:** Rust(`gitim-runtime` crate), `tokio::process::Command` shell out 到 `hermes` CLI

**Non-goals (本计划不做):**
- WebUI 暴露 hermes profile 概念(profile 名由后端推导,前端零感知)
- 多 source profile 选择(永远从 active profile clone)
- profile 重命名 / 跨 agent 迁移(handler 不可变,profile 跟随 handler)
- soft delete 时清理 profile(soft delete 保留所有 agent 数据)
- 已有 agent 的 retroactive profile 创建(只对新加 agent 生效;已有的本计划文档里说明手动迁移路径)

---

## File Structure

### 新建

| 路径 | 职责 |
|------|------|
| `crates/gitim-runtime/src/hermes_profile.rs` | hermes profile 路径计算(`profile_name`/`profile_dir`)、`ensure_profile`(provision 调用)、`delete_profile`(hard delete 调用)、`default_profile_ready`(setup 检测) |
| `crates/gitim-runtime/tests/hermes_profile.rs` | integration tests:profile 创建幂等性、删除 best-effort、default ready 检测 — 需真实 hermes binary,标记 `#[ignore]` |
| `docs/plans/hermes-profile-isolation/plan.md` | 本计划(已存在) |

### 修改

| 路径 | 改动概要 |
|------|----------|
| `crates/gitim-runtime/src/lib.rs` | 暴露 `pub mod hermes_profile` |
| `crates/gitim-runtime/src/agent_loop.rs` (around line 107-110, `with_config`) | 构造 `ProviderConfig` 时,若 `provider_type == "hermes"`,把 `HERMES_HOME` 塞进 `env`(允许 me.json 显式 env 覆盖) |
| `crates/gitim-runtime/src/http.rs` (around line 1211, `add_agent` flow) | me.json 写完后,若 `provider == "hermes"`,先调 `default_profile_ready`,再调 `ensure_profile`;失败走 `cleanup_agent_dir` + 返回 actionable 错误 |
| `crates/gitim-runtime/src/http.rs` (around line 1849-1879, `hard_delete` flow) | hard delete 时调 `delete_profile`(best-effort,失败仅 `tracing::warn`,不阻塞 agent 删除) |
| `crates/gitim-runtime/src/preflight.rs` (line 823-951) | `preflight_hermes_with` 增加 `hermes_home: Option<&Path>` 参数,spawn 时注入 env;`preflight_hermes` 保持原签名传 None |
| `crates/gitim-runtime/tests/preflight_hermes.rs` | 既有测试签名更新;新增"profile 路径生效"测试 |
| `CLAUDE.md` | Current Orientation 段落加一句 hermes profile 隔离机制简介;Non-goals 加 profile 迁移条 |

---

## Phase 0: Baseline 全量测试

确认 main 是绿的,排除祖传红测试干扰后续判断。

### Task 0.1: 全量测试 baseline

- [ ] **Step 1:** 切到 worktree 根,跑 `cargo test --workspace --exclude gitim-runtime` (排除 runtime 是因为它 spawn 真实 daemon 慢)。记录哪些 ignore 测试 / 哪些 fail。
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --lib`(只 lib 测试,跳过 integration)。记录结果。
- [ ] **Step 3:** 跑 `cargo test -p gitim-runtime --test preflight_hermes -- --include-ignored`,确认 hermes binary 在本机存在 + handshake 真能跑通(后续 Phase 5 要用)。如果不通,先跟 user 确认本机 hermes 状态。
- [ ] **Step 4:** baseline 信息写到本计划顶部一个临时 "Baseline as of YYYY-MM-DD" 段落,包含:已知红测试列表、hermes binary 是否就绪。任务结束删除这段。

---

## Phase 1: hermes_profile 模块基础

让一个**手动建好**的 hermes profile 能被 gitim agent loop 使用。这一阶段结束后,如果用户手动 `hermes profile create gitim-foo --clone`,gitim 的 hermes agent 就该走那个 profile。

### Task 1.1: profile 路径计算

**Files:**
- Create: `crates/gitim-runtime/src/hermes_profile.rs`
- Modify: `crates/gitim-runtime/src/lib.rs`(暴露模块)
- Test: 内联 `#[cfg(test)]` (纯 unit, 无 IO)

- [ ] **Step 1: 写失败测试**
  在 `hermes_profile.rs` 内联 mod tests,写两个 unit test:
  - `profile_name_for_alice` 断言 `profile_name("alice") == "gitim-alice"`
  - `profile_dir_for_alice` 断言 `profile_dir("alice")` 等于 `dirs::home_dir() / ".hermes/profiles/gitim-alice"`(注意跨平台 — Linux/macOS 路径行为一致即可,本计划不支持 Windows)
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime hermes_profile -- --nocapture`,验证编译失败(模块/函数还没建)。
- [ ] **Step 3:** 创建 `hermes_profile.rs`,实现两个 pub fn。`profile_dir` 内部用 `dirs::home_dir()`,缺失时返回 `Result<PathBuf>` 的 Err(home dir 缺失是不可恢复错误)。在 `lib.rs` 加 `pub mod hermes_profile`。
- [ ] **Step 4:** 跑同样命令,确认两个测试通过。
- [ ] **Step 5: Commit**
  `git add crates/gitim-runtime/src/lib.rs crates/gitim-runtime/src/hermes_profile.rs && git commit -m "feat(runtime): add hermes_profile path helpers"`

### Task 1.2: agent_loop 注入 HERMES_HOME

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs:107-110`(`with_config` 构造 `ProviderConfig`)
- Test: 内联 `#[cfg(test)]` 在 `agent_loop.rs`(if exist)or 新建 `tests/agent_loop_provider_env.rs`

- [ ] **Step 1: 写失败测试**
  写一个 unit test `provider_config_for_hermes_injects_home`:构造一个 `AgentLoopConfig{provider_type: "hermes", handler: "alice", env: HashMap::new()}`,断言构造出的 `ProviderConfig.env["HERMES_HOME"]` 等于 `~/.hermes/profiles/gitim-alice` 的字符串形式。再写 `provider_config_for_claude_does_not_inject_home` 断言 claude provider 不注入这个 key。再写 `provider_config_explicit_env_overrides_home` 断言 me.json 显式传 `HERMES_HOME` 时,显式值优先(不被覆盖)。
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime provider_config_for_hermes`,验证失败(env 未注入)。
- [ ] **Step 3:** 修改 `with_config`:在 `provider_config.env` 已克隆 `config.env` 之后,若 `provider_type == "hermes"` 且 `env` 没有 `HERMES_HOME` key,插入 `hermes_profile::profile_dir(&config.handler)?` 转字符串。`profile_dir` 失败(home 缺失)走 `RuntimeError`。
- [ ] **Step 4:** 跑同样测试,确认全过。再跑 `cargo test -p gitim-runtime --lib` 确认没破其他单元测试。
- [ ] **Step 5: Commit**
  `git add -p crates/gitim-runtime/src/agent_loop.rs && git commit -m "feat(runtime): inject HERMES_HOME for hermes agents"`

### Task 1.3: `with_defaults` / `with_provider` 入口也注入 HERMES_HOME

**背景:** baseline 检查发现 `agent_loop.rs` 实际有三个入口 — `with_defaults` (生产: `bin/runtime.rs`)、`with_provider`(被 `with_defaults` 内部调,以及 test)、`with_config`(生产: `http.rs` add_agent 路径)。Task 1.2 只改了 `with_config`,前两个入口的 hermes provider 仍然不会注入 HERMES_HOME。三处都要 cover,且应该走同一个 helper(单点来源,避免后续分叉)。

**Files:**
- Modify: `crates/gitim-runtime/src/agent_loop.rs:50-92`(`with_defaults`、`with_provider`)

- [ ] **Step 1:** 在 `agent_loop.rs` 内提取一个 private helper(建议名 `build_provider_config(provider_type: &str, handler: &str, extra_env: HashMap<String, String>) -> Result<ProviderConfig, RuntimeError>`),封装 Task 1.2 的注入逻辑:克隆 extra_env,hermes provider 时插入 HERMES_HOME(若用户 env 没显式传)。
- [ ] **Step 2:** 把 Task 1.2 在 `with_config` 内的内联逻辑改为调用这个 helper(确保行为不变,Task 1.2 的测试仍通过)。
- [ ] **Step 3:** `with_provider` 的 `ProviderConfig::default()` 替换为调 helper(extra_env 传 `HashMap::new()`)。`with_defaults` 已经委托 `with_provider`,自动受益,不用改。
- [ ] **Step 4:** 加一个 unit test `with_provider_for_hermes_injects_home`,直接调 `AgentLoop::with_provider(&tmp, "hermes", "alice")`,assert provider 能被构造(成功创建即说明 ProviderConfig 路径走通)。手动验证:用 `tracing` 在 helper 里打 debug 日志,跑测试看 HERMES_HOME 注入。或更简单:让 helper 暴露成 `pub(crate)` 测试直接断言其返回值。
- [ ] **Step 5:** 跑 `cargo test -p gitim-runtime --lib`,确认所有现有 + 新增测试通过。
- [ ] **Step 6: Commit**
  `git commit -am "refactor(runtime): centralize ProviderConfig construction with profile injection"`

---

## Phase 2: provision 自动建 profile

让 add_agent 在 me.json 写完后自动 ensure profile,user 不需要手动跑 `hermes profile create`。

### Task 2.1: ensure_profile 实现

**Files:**
- Modify: `crates/gitim-runtime/src/hermes_profile.rs`
- Test: `crates/gitim-runtime/tests/hermes_profile.rs` (integration, `#[ignore]` 标记需要真实 hermes)

- [ ] **Step 1: 写失败测试**
  integration test 三个 case(都 `#[ignore]`,需手动跑):
  - `ensure_profile_creates_new` — 用一个临时 handler `gitim-test-XXXX`(随机后缀防撞),调用 `ensure_profile(handler).await`,断言返回 `Ok(EnsureOutcome::Created)`,断言 `profile_dir(handler)` 路径存在,断言里面有 `config.yaml`。end:cleanup 调 `delete_profile`(Task 2.2 后才完整,可先用 `std::fs::remove_dir_all`)。
  - `ensure_profile_idempotent` — 连续调 `ensure_profile` 两次,第二次返回 `Ok(EnsureOutcome::AlreadyExists)`。
  - `ensure_profile_fails_when_no_hermes` — 临时把 PATH 改空(或用一个 mock binary 替换 `hermes`),`ensure_profile` 返回特定 `Err`,错误消息含 actionable 提示("hermes CLI not found")。
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --test hermes_profile -- --include-ignored`,确认编译失败(`ensure_profile` 不存在)。
- [ ] **Step 3:** 在 `hermes_profile.rs` 实现:
  - `pub enum EnsureOutcome { Created, AlreadyExists }`
  - `pub async fn ensure_profile(handler: &str) -> Result<EnsureOutcome, HermesProfileError>` — `tokio::process::Command::new("hermes").args(["profile", "create", &name, "--clone", "--no-alias"]).output().await`,exit 0 → Created;stderr 含 "already exists" → AlreadyExists;PATH 缺失 → `HermesProfileError::CliNotFound`;其他 → `HermesProfileError::Other(stderr_tail)`。
  - `pub enum HermesProfileError` 用 `thiserror` 派生 `Error`/`Display`,在错误消息里写明"请先在终端跑 `hermes setup` 确保 default profile 已配置 provider",这样 add_agent 失败时的 toast 直接 actionable。
- [ ] **Step 4:** 跑测试,确认三个 case 全过。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): implement ensure_profile via hermes CLI shell out"`

### Task 2.2: delete_profile 实现

**Files:**
- Modify: `crates/gitim-runtime/src/hermes_profile.rs`
- Test: `crates/gitim-runtime/tests/hermes_profile.rs`

- [ ] **Step 1: 写失败测试**
  integration test(`#[ignore]`):
  - `delete_profile_removes_existing` — ensure_profile 建一个 → delete_profile 删 → 断言 `profile_dir` 不存在
  - `delete_profile_missing_is_noop` — 直接 delete 不存在的 profile → 返回 Ok(best-effort 语义,不报错)
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --test hermes_profile delete_profile -- --include-ignored`,确认失败。
- [ ] **Step 3:** 实现 `pub async fn delete_profile(handler: &str) -> Result<(), HermesProfileError>` — `hermes profile delete <name> -y`。exit 0 / "does not exist" → Ok;CLI 缺失 → 仅 `tracing::warn` + Ok(best-effort);其他错误 → Err(让调用方决定是否吞)。
- [ ] **Step 4:** 跑测试,确认通过。回头改 Task 2.1 测试的 cleanup 用 `delete_profile`(去掉 `remove_dir_all`)。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): implement delete_profile (best-effort)"`

### Task 2.3: default_profile_ready 检测

**Files:**
- Modify: `crates/gitim-runtime/src/hermes_profile.rs`
- Test: 内联 `#[cfg(test)]`(用 `tempfile` mock home dir)

- [ ] **Step 1: 写失败测试**
  unit test 三个 case:
  - `default_ready_when_env_exists` — mock `HERMES_HOME=<tmpdir>`,在 tmpdir 写个 `.env` → `default_profile_ready` 返回 true
  - `default_ready_when_authjson_exists` — 同上,写 `auth.json` → true
  - `default_not_ready_when_empty` — 空 tmpdir → false
- [ ] **Step 2:** 跑测试,确认编译失败。
- [ ] **Step 3:** 实现 `pub fn default_profile_ready() -> bool` — 解析 `HERMES_HOME` env 或 fallback `~/.hermes`,检查 `.env` 或 `auth.json` 存在(任一即可)。**不**调 hermes binary(open-and-stat 比 spawn 快几十倍)。
- [ ] **Step 4:** 跑测试,确认通过。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): add default_profile_ready setup check"`

### Task 2.4: 接到 add_agent flow

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:1211-1240`(me.json 写完后,state 注册前)

- [ ] **Step 1: 写失败测试**
  现有 add_agent 集成测试(查 `rg "add_agent" crates/gitim-runtime/tests/`)如果有,加一个 case `add_hermes_agent_creates_profile`:
  - workspace ready,POST `/agents` with `provider: "hermes"`
  - 断言 response 200
  - 断言 `~/.hermes/profiles/gitim-<handler>` 存在
  - 测试 cleanup 删 profile
  - 标记 `#[ignore]`(需要 hermes binary)
  
  再加一个 `add_hermes_agent_fails_when_default_not_setup`:
  - mock `HERMES_HOME` 指向空 tmpdir
  - POST `/agents` with `provider: "hermes"`
  - 断言 response 4xx,error 消息含 "hermes setup"
  - 断言 agent 目录被 cleanup(没残留)
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --test <test_file> add_hermes -- --include-ignored`,确认失败。
- [ ] **Step 3:** 在 `http.rs:1211` 处(me.json 写完之后,`AgentInfo` 构造之前)插入:
  - `if req.provider == "hermes"`:
    - 先 `if !hermes_profile::default_profile_ready()` → 走 cleanup + 返回 4xx 含 actionable 文案("请在终端先跑 `hermes setup` ...")
    - 再 `hermes_profile::ensure_profile(&req.handler).await`,失败 → cleanup + 4xx + redacted error
- [ ] **Step 4:** 跑测试,确认两个 case 通过。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): auto-create hermes profile on add_agent"`

---

## Phase 3: hard delete 清理

### Task 3.1: hard_delete 调 delete_profile

**Files:**
- Modify: `crates/gitim-runtime/src/http.rs:1849-1879`(`hard_delete` flow)

- [ ] **Step 1: 写失败测试**
  集成测试:
  - 加 hermes agent → 验证 profile 存在 → POST delete with `hard_delete: true` → 验证 profile 不存在
  - 加 hermes agent → mock hermes binary 改名(模拟用户卸载 hermes 但 profile 还在) → POST delete with hard_delete → response 仍然 200(best-effort 不阻塞)
  - 标 `#[ignore]`
- [ ] **Step 2:** 跑测试,确认失败(profile 没被删)。
- [ ] **Step 3:** 在 `hard_delete_agent_dir` 后(or 调用方 `req.hard_delete` 分支)加:`if 该 agent 的 me.json provider == "hermes"`,异步调 `delete_profile`,失败仅 `tracing::warn`,不影响 response。注意:me.json 已经被 `hard_delete_agent_dir` 删掉了,**provider 信息要在 hard_delete 调用前从 state 或 me.json 读出**,顺序很关键。
- [ ] **Step 4:** 跑测试,确认通过。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): clean up hermes profile on hard delete"`

---

## Phase 4: preflight 一致性

让 preflight 能跑在指定 profile 上,而不是只测 default profile。

### Task 4.1: preflight_hermes_with 加 hermes_home 参数

**Files:**
- Modify: `crates/gitim-runtime/src/preflight.rs:823-951`
- Modify: `crates/gitim-runtime/tests/preflight_hermes.rs`(签名同步)
- Modify: `crates/gitim-runtime/src/http.rs:2275-2276`(调用方传 None 保持原行为)

- [ ] **Step 1: 写失败测试**
  在 `tests/preflight_hermes.rs` 加一个 `#[ignore]` 测试 `test_preflight_hermes_with_custom_home`:
  - 用 `tempdir` 作为 HERMES_HOME
  - 跑 `preflight_hermes_with("hermes", Duration::from_secs(10), Some(tmpdir.path()))`
  - 断言 child process 实际 spawn 时 env 含正确 HERMES_HOME(可以通过 trace log 验证,或包一层 helper 暴露 last_spawn_env 用于测试)
  - 至少要保证不 panic,不 regression default 行为
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --test preflight_hermes -- --include-ignored`,验证 sigfail(参数不存在)。
- [ ] **Step 3:** 改 `preflight_hermes_with` 签名加 `hermes_home: Option<&Path>`,spawn `hermes acp` 时若 Some 则 `cmd.env("HERMES_HOME", path)`。`preflight_hermes` wrapper 传 None。所有现有 test 用例传 None 适配新签名。
- [ ] **Step 4:** 跑全部 preflight 测试 + http.rs 调用方测试,确认通过。
- [ ] **Step 5: Commit**
  `git commit -am "feat(runtime): preflight_hermes_with accepts custom HERMES_HOME"`

### Task 4.2: agent loop 启动前 per-profile preflight — **跳过**

**决策(2026-04-29):跳过本 task**。

**调研发现:**
- `start_agent_loop` 里 `agent_loop.init()` 失败已经把 `status` 置 `"error"`(`http.rs:1389`),WebUI 能看到。这是好的失败暴露路径。
- 真正的"静默死循环"是 `run_once` 持续失败后的指数 backoff(`http.rs:1416-1429`),但这是**所有 provider 共有**的设计,不是 hermes-profile 引入的问题。修它属于 agent_loop 鲁棒性增强(类似 sync_loop 的 auth 熔断),不属于 hermes-profile-isolation 的 scope。
- 加 per-profile preflight 会让 add_agent 体感多等 ~10s(ACP handshake 时长),用户感知差。
- `default_profile_ready`(Task 2.3)已经在 add_agent 阶段阻断"完全没 setup"这种最常见配置错误,剩下的 ACP 失败属于 hermes 自身配置问题(过期 token、模型不可达等),preflight 也救不了。

**未来触发条件:** 如果 user 真的反馈"加完 agent 看着 idle 半天才发现没回话",再回头加这个 preflight 或上"backoff 上限熔断"。短期内不优先。

---

## Phase 5: 文档 + 收尾

### Task 5.1: CLAUDE.md 更新

**Files:**
- Modify: `CLAUDE.md`("Current Orientation" 段落 + 视情况新增"Hermes profile 隔离"小节)

- [ ] **Step 1:** 在 "Current Orientation" 的 "Where we are" 末尾,加一句:"Hermes provider 已支持 per-agent profile 隔离 — 每个 agent 自动获得独立的 `~/.hermes/profiles/gitim-<handler>` 目录,LLM 配置/auth/session 完全隔离,user 只需在 `hermes setup` 一次配 default profile 作为模板"。
- [ ] **Step 2:** 加 "## Hermes profile 隔离机制" 小节,内容:profile 命名约定、provision 流程、hard delete 清理、user 切换 agent LLM 的工作流(`hermes -p gitim-<handler> setup model`)、已知 non-goals(soft delete 不清、retroactive 不补、profile 重命名不支持)。
- [ ] **Step 3:** Tensions 段落加一条:"hermes profile 通过 shell out 调 `hermes profile create/delete`,依赖 hermes CLI 在 PATH;如果 user 升级 hermes 改了 profile 内部结构,我们的 `--clone` 行为会自动跟进,但 `default_profile_ready` 检测的 `.env`/`auth.json` 路径假设可能 drift,需要每个 hermes major 版本回归一次"。
- [ ] **Step 4: Commit**
  `git commit -am "docs(claude): document hermes profile isolation mechanism"`

### Task 5.2: 已有 hermes agent 迁移说明

**Files:**
- Create or Modify: `docs/plans/hermes-profile-isolation/migration.md`

- [ ] **Step 1:** 写一份"已有 hermes agent 迁移指引",一条命令搞定:`for d in <workspace>/.gitim-runtime/agents/*; do handler=$(basename $d); hermes profile create gitim-$handler --clone --no-alias 2>/dev/null || true; done`。说清楚:不需要重启 agent / 不需要改 me.json,下次 agent 启动时 ProviderConfig 会自动注入 HERMES_HOME。
- [ ] **Step 2: Commit**
  `git add docs/plans/hermes-profile-isolation/migration.md && git commit -m "docs(plan): add hermes profile retro-migration guide"`

### Task 5.3: 收尾全量测试

- [ ] **Step 1:** 跑 `cargo test --workspace`(全量,**需要分钟级时间**,接受这次开销)。
- [ ] **Step 2:** 跑 `cargo test -p gitim-runtime --test hermes_profile -- --include-ignored`(需要 hermes binary),验证所有 ignore 测试在真实环境通过。
- [ ] **Step 3:** 手动 e2e:启动 runtime → WebUI add hermes agent "alice" → 验证 `~/.hermes/profiles/gitim-alice` 创建 → 在 webui 发消息,验证 agent 用该 profile 的 LLM 配置回复 → hard delete agent → 验证 profile 删除。
- [ ] **Step 4:** 全过 → 准备 finishing-a-development-branch。
- [ ] **Step 5:** 更新本计划 "Current Orientation" 段落标记完成,删 Phase 0 临时 baseline 段。

---

## 回滚预案

如果 Phase 2/3 上线后发现问题(比如某些 hermes 版本的 `profile create` 行为不一致):

- 临时降级:把 `http.rs` 的 `if req.provider == "hermes"` 分支整段加一个 feature flag 包住(env var 控制),关掉就退回到"不建 profile,所有 agent 共享 default" 的老行为
- 紧急修复:`HERMES_HOME` 注入(Phase 1)是无害的,即使 profile 不存在 hermes 会 fallback 创建空目录,所以最坏情况下功能等价于 v1 但少了 setup 检测

不计划 git revert — 改动覆盖多个 commit,prefer 加 flag 关掉。

---

## 已知 Tensions / 留给后续的事

- `hermes profile create --clone` 的源是 active profile,如果 user 跑过 `hermes profile use foo` 切了 active,gitim agent 会从 foo 而不是 `~/.hermes` clone。文档里说明,v1 不引入 source 选择 UI。
- `delete_profile` 是 best-effort,如果 hermes binary 临时不可用,profile 残留在磁盘。下次同 handler 重新 add_agent 会撞 `AlreadyExists`,但 `ensure_profile` 处理为幂等,不会失败。
- handler 重命名 / 跨机器同名冲突:handler 在 gitim 是 immutable 且 daemon 已有冲突防护,profile 跟随 handler 没有额外问题。
- 目前 webui 看不到"哪个 agent 用了哪个 profile"。v2 可以在 agent detail 页面加只读字段显示 profile 名 + 一个"在终端打开 hermes 配置"的深链按钮。
