# WebUI Update-and-Restart Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** WebUI 右上角检测到新版本时显示黄色 ⚠ 叹号,hover 提示、点击弹出 HoverCard,里面一个 "Update & Restart" 按钮。点击后 runtime 自己下载 latest release tarball,替换 `~/.gitim/bin/` 下的三个二进制,kill 自管 daemons,fork-exec 新 runtime,老进程退出。前端轮询 `/health` 感知新版本上线,toast 反馈后触发数据 refetch。

**Architecture:** 新建 `gitim-updater` 共享 crate,收拢 reqwest/flate2/tar 依赖,暴露细粒度的纯函数 + I/O 函数;`gitim-cli::commands::update` 和 runtime 新 endpoint `POST /runtime/update-and-restart` 各自 orchestration。Runtime 启动时缓存 canonical `current_exe` 路径,严格模式只允许从 `~/.gitim/bin/` 启动的进程升级。替换文件前 `rename → .old` 备份,失败回滚;fork-exec 用缓存路径 spawn 新 runtime 进程,老进程 exit(0)。新 runtime 走现有 `recover_from_config` 流程重建 daemons,daemons spawn 时优先用 `current_exe().parent()/gitim-daemon`,避免 PATH 里其他版本污染。

**Tech Stack:** Rust(新 crate + runtime 改动 + CLI refactor + client 改动),React / TypeScript(webui-v2 新 banner + hook 重构)

---

## 设计决策速览(来自 grill 会话 + eng review)

| # | 决策 | 选项 |
|---|------|------|
| Q1 | 语义 = **一键升级 + 重启**,不是纯重启 | 纯重启对用户无意义,除非先更新二进制 |
| Q2 | restart 机制 = runtime **fork-exec** 新进程;旧进程 `exit(0)` | 不改 execve、不做外挂 supervisor |
| Q3 | 升级范围 = **三个 binary 一起换**(`gitim` / `gitim-daemon` / `gitim-runtime`) | 避免 runtime 新版 / CLI 旧版的版本分裂 |
| Q4 | 代码位置 = 新 crate **`gitim-updater`**,只 expose 细粒度函数 | CLI 和 runtime 各自封 orchestration |
| Q5 | HTTP 契约 = `POST /runtime/update-and-restart`,同步阶段 202 / 异步 background task | 同步阶段包含 download + extract + `--version` sanity check |
| Q6 | 前端入口 = header 右侧 **黄色 ⚠ 叹号**,hover tooltip、click 展开 **HoverCard** | 不可 dismiss,无 release notes 链接 |
| Q7 | restart 窗口 UX = spinner + 状态机("Updating" → "Restarting")+ `isUpdating` flag 静默其他 API 错误 | 30s 超时 → 红色错误态 + toast |
| Q8 | install dir 严格模式 = 只有从 `~/.gitim/bin/` 启动的 runtime 才允许升级 | 非此路径 → 403 `runtime_not_installed`,按钮置灰 |
| Q9 | CSRF 姿态 = 继承现状 `CorsLayer::permissive()`,不为此 endpoint 破例 | 整站收紧另立 issue |
| Q10 | daemon PATH 一致性 = `gitim-client::spawn_daemon` 优先 sibling path | 避免"runtime 新 / daemon 旧"的升级幻觉 |

**Eng review 新增 baked-in 定调:**

- **A1 cached canonical exe path**:runtime `run_shell` 启动时立刻 `std::env::current_exe()?.canonicalize()?` 存到 `RuntimeState`,install-dir strict check 和 fork-exec 目标路径都用这个缓存值
- **A3 PID file write 挪到 `run_shell`**:现在 `bin/runtime.rs::daemonize` 里写 PID,restart 的 fork-exec 子进程不会走 daemonize → PID 不更新。必须把 PID write 移到 `run_shell()` 开头,所有入口统一
- **Failure mode 防护**:`replace_binaries` 每个文件 unlink 前先 `rename → <name>.old` 备份;任一文件失败 → 回滚所有已替换;全成功 → 清理 `.old`
- **T1 E2E test**:写 mock Cell API + mock release server 的集成测试,覆盖 "POST endpoint → 下载 → 替换 → fork-exec → 新进程 serve 新 /health" 全链路

---

## Non-goals (v1)

明确**不**做以下场景:

| 不做 | 原因 | 用户 workaround |
|------|------|----------------|
| Dev-mode 升级(非 `~/.gitim/bin/` 启动) | 会破坏 `cargo run` / `cargo install` 工作流 | 开发者在终端 `cargo build` 自己管 |
| 自定义版本选择 / 降级 | UI 复杂度不划算 | 终端 `gitim update <version>` 绕过 |
| Release notes 显示 | Cell API 不稳定提供 | 用户自己访问 GitHub releases 页 |
| 真下载进度条 | Cell API 返回无 Content-Length 保证,scope 爆 | indeterminate spinner |
| 新 runtime 启动失败的自动 rollback | 需要 A/B binary slot + 握手机制 | 终端 `gitim runtime start` 手动恢复;plan 里记 future work |
| 自动后台升级 | 只做用户主动触发 | — |
| Windows 支持 | 与 `install.sh` 保持一致 | Mac / Linux 为第一优先级 |
| Banner dismiss | dev 工具不是消费级 app,不骚扰 | banner 消失要么升级要么卸载 |
| CSRF 整改 | 整站问题,不 scope 为此功能解决 | Known Risks 记录,另立 issue |

---

## What already exists(避免重造)

| 已有能力 | 位置 | 用法 |
|---------|------|-----|
| CLI `gitim update` 命令 | `crates/gitim-cli/src/commands/update.rs` | 下载 / 解压 / 替换逻辑抽到新 crate,CLI refactor 成调用方 |
| Platform 检测 | 同上 | `detect_platform()` 迁入新 crate |
| 版本比较 | 同上 | `parse_version` + `is_newer` 迁入新 crate |
| GitHub Release API URL 构造 | 同上 | `download_url` + `latest_release_api_url` 迁入新 crate |
| `install.sh` | 根目录 | install dir `~/.gitim/bin/` 是 source of truth(strict check 基准) |
| `release.sh` + `CiferaTeam/gitim-releases` | 根目录 + GitHub | 已有发布流程,plan 无需动 |
| `GET /health` 返 `version` | `crates/gitim-runtime/src/http.rs` 约 L168-175 | 前端比较 current vs latest 用 |
| `use-version-check` hook | `webui-v2/src/hooks/use-version-check.ts` | 当前只拉 Cell API 并丢弃结果,refactor 为完整比较 hook |
| Cell API `/api/check-version` | `webui-v2/src/lib/cell-api.ts` | 返 `latest_version` 仍走现有契约 |
| `daemonize()` / `run_shell()` | `crates/gitim-runtime/src/bin/runtime.rs` | fork-exec 模式参考;PID write 要从前者挪到后者 |
| `recover_from_config` | `crates/gitim-runtime/src/http.rs` | 新 runtime 启动后自动 re-provision workspaces + daemons |
| `kill_managed_daemons` | 同上 | 异步阶段调用,SIGTERM 现有的 daemon 关停逻辑复用 |
| `CorsLayer::permissive()` | `crates/gitim-runtime/src/http.rs:2362` | 继承现状,endpoint 同其它一视同仁 |
| Sonner toast | `webui-v2/src/app.tsx` | 成功 / 错误 / 超时 toast 复用 |
| Radix `HoverCard` | Radix UI 已引入 | banner 展开卡片复用 |
| `useConnectionStore` | `webui-v2/src/store/connection.ts`(推测路径,实际 verify) | 新增 `isUpdating` / `isRestarting` state 挂这里 |

---

## 文件结构

**新增:**

- `crates/gitim-updater/Cargo.toml`
- `crates/gitim-updater/src/lib.rs` — pure helpers + IO helpers + `UpdateError` enum
- `crates/gitim-updater/tests/helpers.rs` — version / platform / URL unit tests
- `crates/gitim-updater/tests/download_replace.rs` — mock HTTP + crafted tarball + replace_binaries + backup-restore
- `crates/gitim-runtime/src/update.rs` — update endpoint 的 orchestrator(调用 updater crate helpers,编排 sync/async phase)
- `crates/gitim-runtime/tests/update_handler.rs` — 同步阶段各错误路径 + 202 happy path
- `crates/gitim-runtime/tests/update_e2e.rs` — full flow 集成测试(mock Cell API + mock release server + fake binaries)
- `webui-v2/src/components/update-indicator.tsx` — 黄色叹号 + HoverCard + Update button 的完整组件
- `webui-v2/src/hooks/use-version-check.ts` 的测试(若 webui-v2 有测试基建,否则跳过 / 只做手工验证)

**修改:**

- `Cargo.toml`(根)— 把 `gitim-updater` 加到 workspace members
- `crates/gitim-cli/Cargo.toml` — 移除直接的 reqwest / flate2 / tar 依赖,替换为 `gitim-updater` 依赖
- `crates/gitim-cli/src/commands/update.rs` — refactor 成新 crate 的薄封装(保留 CLI 交互层:confirm prompt / daemon 停止提示 / 状态输出)
- `crates/gitim-runtime/Cargo.toml` — 加 `gitim-updater` 依赖
- `crates/gitim-runtime/src/lib.rs` — 注册 `pub mod update;`
- `crates/gitim-runtime/src/http.rs` — 挂载新 route + 在 `RuntimeState` 里加 canonical exe path + 加 `update_status` tracker
- `crates/gitim-runtime/src/bin/runtime.rs` — (1) 把 PID file write 从 `daemonize()` 移到 `run_shell()` 开头;(2) `run_shell()` 启动时缓存 `current_exe().canonicalize()` 到 state;(3) 接受新的 arg `--restarted-from-update`(可选,仅用于日志标记)
- `crates/gitim-client/src/daemon.rs` — `spawn_daemon` 先试 `current_exe().parent()/gitim-daemon`,不存在再退回 PATH 解析;加 unit tests
- `webui-v2/src/lib/client.ts` — (1) 新增 `updateAndRestart()` API 调用;(2) 新增 `getHealth()` API 调用(已有则复用);(3) 在 `isUpdating` 期间静默 swallow fetch 错误(不弹网络错误 toast)
- `webui-v2/src/lib/cell-api.ts` — 不改契约,只是现在有消费者了
- `webui-v2/src/hooks/use-version-check.ts` — 重构为合并 hook:同时读 Cell API latest + runtime `/health` current,返回 `{ current, latest, hasUpdate, isUpdating, error }`;升级流程的状态机也在这里驱动
- `webui-v2/src/components/layout/app-shell.tsx` — 在 header 右侧 help 图标**左侧**嵌入 `<UpdateIndicator />` 组件
- `webui-v2/src/store/connection.ts`(或等价的 zustand store)— 新增 `isUpdating: boolean` + `isRestarting: boolean` + setters

---

## Tasks

### Task 1:创建 `gitim-updater` crate,迁入纯函数

**Files:**
- 新增 `crates/gitim-updater/Cargo.toml`
- 新增 `crates/gitim-updater/src/lib.rs`
- 新增 `crates/gitim-updater/tests/helpers.rs`
- 修改根 `Cargo.toml` 把 `gitim-updater` 加到 workspace members

**Steps:**

- [ ] 新 crate 的 `Cargo.toml` 声明依赖:`reqwest`(rustls-tls + json)、`flate2`、`tar`、`thiserror`、`tokio`(workspace)、`serde_json`(workspace)。`version.workspace = true`
- [ ] `src/lib.rs` 实现:`UpdateError` 枚举(variants:`unsupported_platform`、`network`、`http_status`、`extract`、`missing_binary`、`io`)+ `BINARIES` 常量 + `parse_version` + `is_newer` + `detect_platform` + `download_url` + `latest_release_api_url` 纯函数
- [ ] `tests/helpers.rs` 写足 unit 测试:`parse_version` 覆盖 空串 / `"1.2.3"` / `"v1.2.3"` / `"v0.10.0"` / `"bad"` / `"1.2"`(6 cases);`is_newer` 覆盖 older/same/newer/malformed(4 cases);`download_url` 格式合约(1 case);`latest_release_api_url` 格式合约(1 case);`detect_platform` 在测试主机成功返回字符串
- [ ] `cargo test -p gitim-updater` 全绿

**验收:**
- 新 crate 能被 workspace 识别、编译通过、所有纯函数 unit 测试通过
- `cargo tree -p gitim-updater` 不含 `openssl`(确认走 rustls)

---

### Task 2:给 `gitim-updater` 加 IO helpers + 测试

**Files:**
- 修改 `crates/gitim-updater/src/lib.rs`
- 新增 `crates/gitim-updater/tests/download_replace.rs`

**Steps:**

- [ ] 在 `lib.rs` 加 async `fetch_latest_tag()`:GET `latest_release_api_url()`,user-agent `"gitim-updater"`,JSON 解析 `tag_name` 字段
- [ ] 加 async `download_and_extract(url: &str, dest: &Path)`:reqwest 流式读取 bytes,flate2 + tar 解压到 dest
- [ ] 加 sync `find_binary(dir: &Path, name: &str) -> Option<PathBuf>`:递归查找(沿用 CLI 现有 `walkdir` 私有实现,移过来)
- [ ] 加 sync `replace_binaries(src_dir: &Path, install_dir: &Path, keep_backup: bool) -> Result<Vec<String>, UpdateError>`:对每个 `BINARIES`,`find_binary` 定位源文件 → 目标存在则 `rename` 到 `<target>.old` 做备份 → `copy` 新文件 → `chmod 0o755`。任一失败 → 回滚(把所有已经 rename 的 `.old` 恢复)+ 返回 Err。全成功:若 `keep_backup` 为 false 则删除所有 `.old`
- [ ] `tests/download_replace.rs` 写集成测试:
  - download_and_extract:起本地 `axum` / `wiremock` mock server,serve 一个预制 tarball,验证解压后目录结构符合预期
  - replace_binaries happy path:mock src_dir 放三个 shell-script 假 binary,replace 后 install_dir 里文件内容 = src_dir 内容、权限 = 0o755、无遗留 `.old`
  - replace_binaries 缺一个 binary:只有两个假 binary,replace 应跳过缺失的那个 + warn + 其它成功
  - replace_binaries 回滚:人为让第二个 binary 的 copy 失败(例如 install_dir 的 `gitim` 设为只读),验证前一个 binary 从 `.old` 恢复、整体返回 Err
- [ ] `cargo test -p gitim-updater` 全绿

**验收:**
- 所有 IO helpers 有测试覆盖,包括错误回滚路径
- `replace_binaries` 保证原子性:要么全部替换,要么全部保持旧版

---

### Task 3:Refactor CLI `gitim update` 使用新 crate

**Files:**
- 修改 `crates/gitim-cli/Cargo.toml`
- 修改 `crates/gitim-cli/src/commands/update.rs`

**Steps:**

- [ ] `Cargo.toml` 移除 `reqwest` / `flate2` / `tar` 直接依赖(保留 `tempfile`),新增 `gitim-updater = { path = "../gitim-updater" }`
- [ ] `commands/update.rs` 删掉所有迁到新 crate 的函数(`parse_version`、`is_newer`、`detect_platform`、`download_url`、`latest_release_api_url`、`fetch_latest_tag`、`download_and_extract`、`find_binary`、`walkdir`、`replace_binaries`、`BINARIES`)
- [ ] 保留 CLI 专属的交互层:`confirm()` 提示、`cmd_update()` 主流程(现在调用 `gitim_updater::*`)
- [ ] 调用 `replace_binaries(..., keep_backup = false)` —— CLI 模式不需要保留备份
- [ ] 保留原有的 daemon-running 检测 + 停止提示(`find_repo_root` + `is_daemon_running`)
- [ ] `cargo test -p gitim-cli` 全部通过;所有原有的 update tests 保持相同行为
- [ ] 手工 smoke:`cargo run -p gitim-cli -- update --help` 输出正常

**验收:**
- CLI 的 `gitim update` 外在行为与 refactor 前完全一致(--help 输出、各错误分支、交互提示)
- `crates/gitim-cli/Cargo.toml` 不再直接 import reqwest / flate2 / tar
- 无代码重复:CLI `commands/update.rs` 只编排,不含下载 / 解压 / 替换逻辑

---

### Task 4:`gitim-client::spawn_daemon` 优先用 sibling path

**Files:**
- 修改 `crates/gitim-client/src/daemon.rs`

**Steps:**

- [ ] 在 `spawn_daemon` 内部,`Command::new("gitim-daemon")` 前先尝试 `std::env::current_exe()?.canonicalize()?.parent()?.join("gitim-daemon")`;若该路径存在且可执行,`Command::new(<sibling_path>)`;否则 fallback 现有 PATH 解析
- [ ] 抽 helper `resolve_daemon_binary() -> PathBuf`(pub(crate)),容易测试
- [ ] 在同文件 `#[cfg(test)]` mod 加 unit tests:
  - current_exe parent 有 `gitim-daemon` 文件 → resolve 返回 sibling path
  - current_exe parent 无 `gitim-daemon` → resolve 返回 `"gitim-daemon"`(PATH 解析基准)
  - `current_exe()` 失败(mock)→ resolve fallback 为 `"gitim-daemon"`
- [ ] `cargo test -p gitim-client` 全绿

**验收:**
- install dir `~/.gitim/bin/` 里同时有 `gitim-runtime` + `gitim-daemon` 时,runtime spawn 的 daemon 是同目录下的版本,不受 PATH 次序影响
- 开发机 cargo test 时 current_exe 在 target/debug,没有 sibling gitim-daemon,fallback 到 PATH 解析(这是测试环境的常态)

---

### Task 5:Runtime PID file write 迁移 + canonical exe 缓存

**Files:**
- 修改 `crates/gitim-runtime/src/bin/runtime.rs`
- 修改 `crates/gitim-runtime/src/http.rs`(`RuntimeState` 加字段)

**Steps:**

- [ ] `bin/runtime.rs::daemonize()` 删除 PID write 那段;只保留 fork-spawn 逻辑
- [ ] `bin/runtime.rs::run_shell()` 开头加 PID write:`std::fs::write("~/.gitim/runtime.pid", std::process::id().to_string())`(注意路径展开)
- [ ] `run_shell()` 开头加 canonical exe path 缓存:`let canonical_exe = std::env::current_exe()?.canonicalize()?` → 塞进 `RuntimeState`(新字段 `canonical_exe_path: PathBuf`)
- [ ] `http.rs` 的 `RuntimeState` 结构加 `pub canonical_exe_path: PathBuf` 字段(或 `Option<PathBuf>`,Runtime 推荐 eager 初始化)
- [ ] `create_router()` 默认值需要调整(或工厂模式让调用方必须传)
- [ ] `recover_from_config` / 其他已有调用方无改动
- [ ] 现有的 runtime 集成测试(`tests/provision.rs` / `tests/poller.rs`)仍全绿

**验收:**
- 启动 runtime → PID file 存在且内容 = 当前 PID
- `kill <pid>` 后重启 runtime → PID file 被覆写为新 PID,无残留旧 PID
- `RuntimeState.canonical_exe_path` 存在且是 canonicalized 的绝对路径

---

### Task 6:Runtime update endpoint — 同步阶段

**Files:**
- 新增 `crates/gitim-runtime/src/update.rs`
- 修改 `crates/gitim-runtime/src/lib.rs`(`pub mod update;`)
- 修改 `crates/gitim-runtime/src/http.rs`(挂载 route)
- 修改 `crates/gitim-runtime/Cargo.toml`(加 `gitim-updater`)
- 新增 `crates/gitim-runtime/tests/update_handler.rs`

**Steps:**

- [ ] `Cargo.toml` 加 `gitim-updater = { path = "../gitim-updater" }`
- [ ] `update.rs` 定义 `UpdateJob { job_id: Uuid, target_version: String, started_at: ... }` response 结构 + error response 结构(含 `error_code` + `detail`)
- [ ] 实现同步阶段 handler `update_and_restart(State, Json)`:
  1. `strict_install_dir_check(&state.canonical_exe_path)` — 对比 canonical `~/.gitim/bin/`(展开 `dirs::home_dir`),不匹配返回 403 `runtime_not_installed`
  2. `gitim_updater::detect_platform()` 失败 → 400 `unsupported_platform`
  3. `fetch_latest_tag()` 失败 → 500 `network`
  4. 比较 latest vs 当前 `env!("CARGO_PKG_VERSION")`,若不 newer → 400 `already_latest`(或直接 200 无动作,由 plan review 细化;推荐 400 + code)
  5. 建 tempdir → `download_and_extract` → 失败 → 500 `download_failed` / `extract_failed`
  6. 验证 tempdir 里 `find_binary` 三个 binary 都在 → 缺失 → 500 `archive_missing_binaries`
  7. 对 tempdir 里的新 `gitim-runtime` 执行 `--version`(短 timeout 2s),退出码非 0 或输出不含 target version → 500 `sanity_check_failed`
  8. 全通过 → spawn 异步阶段 task(见 Task 7)→ 同步返回 202 `{ job_id, target_version, started_at }`
- [ ] route 挂 `/runtime/update-and-restart` `POST`
- [ ] `tests/update_handler.rs` 写集成测试覆盖上面 1-7 的每个错误分支;happy path 的 202(此时异步阶段 mock 掉或先跳过,Task 7 再补)
- [ ] `cargo test -p gitim-runtime --test update_handler` 全绿

**验收:**
- 所有同步错误分支有测试,错误 `error_code` 枚举清晰
- 同步阶段不做任何 filesystem mutation,任一错误返回时 install dir 未改动

---

### Task 7:Runtime update endpoint — 异步阶段 + E2E 测试

**Files:**
- 修改 `crates/gitim-runtime/src/update.rs`
- 新增 `crates/gitim-runtime/tests/update_e2e.rs`

**Steps:**

- [ ] 实现异步阶段 background task(由 Task 6 的 handler spawn):
  1. `kill_managed_daemons(&state)` — SIGTERM + 短 grace → 超时 fallback SIGKILL(复用或参考现有 signal handler 的路径)
  2. `replace_binaries(tempdir, install_dir, keep_backup = true)` — 失败时自动回滚 + state 记录错误
  3. spawn 新 runtime:`Command::new(&state.canonical_exe_path).args(["--port", &port]).spawn()`(port 从 state 读,当前绑定端口)。失败 → state 记录错误,老进程保持存活
  4. spawn 成功 → 清理 `.old` 备份 → 当前进程 `std::process::exit(0)`
- [ ] `update_e2e.rs` 写完整 E2E 测试:
  - 起 local mock server serve:`/repos/CiferaTeam/gitim-releases/releases/latest`(返 fake tag)+ tarball 下载 URL(返预制的 fake-binary tarball,里面三个 shell script 作为 fake binary,`gitim-runtime --version` 返特定字符串)
  - 起 runtime 实例,`canonical_exe_path` 指到 test temp install dir,install dir 放旧 fake-binary
  - 用 reqwest 发 `POST /runtime/update-and-restart` → 收 202
  - 轮询 `GET /health`,直到新进程响应且 `version` = target version(带 30s timeout)
  - 验证 install dir 里三个文件是新版本 + 没有 `.old` 残留
- [ ] `cargo test -p gitim-runtime --test update_e2e` 全绿(可能需要 `serial_test` 避免端口冲突)

**验收:**
- E2E 测试从 POST 到新 runtime serve 新 version 全链路通过
- 异步阶段失败时老进程不死,state 里能查到错误(供前端轮询或另起查询 endpoint 用 —— v1 前端不查,只超时)

---

### Task 8:Frontend `use-version-check` hook 重构

**Files:**
- 修改 `webui-v2/src/hooks/use-version-check.ts`
- 修改 `webui-v2/src/lib/client.ts`(加 `getHealth` + `updateAndRestart`)
- 修改 `webui-v2/src/store/connection.ts`(或等价 store,先 grep 实际位置)

**Steps:**

- [ ] `client.ts` 加 `async function getHealth(): Promise<{ version: string, ... }>` — GET `/health`
- [ ] `client.ts` 加 `async function updateAndRestart(): Promise<{ job_id, target_version } | { error_code, detail }>` — POST `/runtime/update-and-restart`
- [ ] `client.ts` 的 fetch wrapper 新增:读 `isUpdating` / `isRestarting` store state,若为 true 则 fetch 错误 swallow(不弹 toast,只 log)
- [ ] store 加 `isUpdating: boolean` + `isRestarting: boolean` + setters
- [ ] `use-version-check.ts` 重构:
  - 启动时并行 `getHealth` + Cell API `checkVersion` → 合并为 `{ current, latest, hasUpdate }`
  - 保持 1h 轮询节奏 + on-mount 触发
  - 暴露 action `triggerUpdate()`:调 `updateAndRestart` → 成功 202 → set `isUpdating=true` → 开始 500ms 轮询 `getHealth` → 连接失败 → set `isRestarting=true` → 重连后 version 匹配 target → set 全 false + toast "Updated to v{x}" + 触发 workspace / channel 数据 refetch → 若 30s 超时未成功 → set 错误态 + toast "Update may have failed"
  - 返回 `{ current, latest, hasUpdate, isUpdating, isRestarting, error, triggerUpdate }`
- [ ] 若 webui-v2 有 vitest/jest 基建,补 hook 的 happy / error / timeout 测试(若无,手工验)

**验收:**
- `useVersionCheck()` 在不同版本场景下(latest > current / latest == current / /health 失败 / Cell API 失败)返回正确 `hasUpdate`
- `triggerUpdate()` 的状态机走通完整流程;30s 超时有对应错误态
- 升级期间其他页面的 API 调用不弹误导性错误 toast

---

### Task 9:Frontend `UpdateIndicator` 组件

**Files:**
- 新增 `webui-v2/src/components/update-indicator.tsx`
- 修改 `webui-v2/src/components/layout/app-shell.tsx`

**Steps:**

- [ ] `update-indicator.tsx` 实现:
  - hook in `useVersionCheck()`
  - `hasUpdate === false && !isUpdating && !isRestarting` → 不渲染任何东西
  - 黄色 ⚠ icon(Tailwind `text-amber-500` 或等价 + lucide 或 radix icon)
  - Radix `HoverCard`:trigger 是 icon,content 内容三行("New version v{latest} available" / "You're on v{current}" / `<Button>` "Update & Restart")
  - `isUpdating && !isRestarting` → icon 变 spinner,HoverCard content 显示 "Updating..." + 禁用按钮
  - `isRestarting` → icon 仍 spinner,content 显示 "Restarting..."
  - error 状态 → icon 变红色、content 显示错误文案
- [ ] `app-shell.tsx` 在 header 右侧(`justify-end` 那一块),help 图标**左侧**嵌入 `<UpdateIndicator />`
- [ ] 视觉对齐:icon 大小匹配 help / user 的大小(参考现有 header 尺寸),间距跟随 header 现有 `gap-2`
- [ ] 主题适配:light/dark mode 下 ⚠ 可见度都 OK(若项目只有 dark mode,按 dark 调)
- [ ] 手工 smoke test:Vite dev server 起来,mock Cell API 返 fake latest(可临时改 env var),验证:
  - 无更新 → icon 不显示
  - 有更新 → 黄色 ⚠ 显示,hover 出 HoverCard,按钮可点
  - 点按钮后显示 spinner

**验收:**
- Banner 在 header 正确位置,视觉风格与现有 UI 协调
- 各状态(闲置 / 有更新 / updating / restarting / error)渲染正确
- HoverCard 交互不影响其他 header 元素

---

### Task 10:Known Risks 文档 + future work TODO

**Files:**
- 修改 `docs/plans/2026-04-19-webui-update-and-restart.md`(本文件)— 添加 Known Risks 章节(已在下方)
- 考虑在 `CLAUDE.md` 的 "Current Orientation" 里加一行说明新能力(可选,看情况)

**Steps:**

- [ ] 确认本文档 `Known Risks` 章节(见下)准确反映两个已知限制
- [ ] 在 commit message 里点明未 fix 的两个 risk(CSRF 继承、新 runtime 启动失败无 auto-recover)

**验收:**
- `docs/plans/` 里的文档充分说明 limitations,未来维护者能看明白

---

## Test Strategy 汇总

| 层级 | 数量估算 | 类型 | 位置 |
|------|--------|------|------|
| `gitim-updater` 纯函数 | 12+ | unit | `tests/helpers.rs` |
| `gitim-updater` IO 函数 | 4 | integration | `tests/download_replace.rs` |
| `gitim-client::spawn_daemon` | 3 | unit | `daemon.rs` 内 `#[cfg(test)]` |
| Runtime update handler 同步阶段 | 6-7 | integration | `tests/update_handler.rs` |
| Runtime update 全流程 E2E | 1 | integration | `tests/update_e2e.rs` |
| Frontend `use-version-check` hook | 4+(若有基建) | unit | vitest/jest |
| Frontend `UpdateIndicator` 组件 | 5+(若有基建) | render | 同上 |

**关键 E2E 测试需要:**
- 本地 axum / wiremock mock server
- 预制 tar.gz 包含三个 shell-script "fake binary",其中 fake `gitim-runtime --version` 输出 target version,`--port XXXX` 模式下 bind 到指定端口并 serve 最简 `/health`
- 用 `serial_test` 防并发端口冲突

**不测但接受风险:**
- Cell API 真实网络调用(mock 掉)
- GitHub Releases API 真实调用(mock 掉)
- macOS code signing 验证(runtime 被替换后是否能启动 —— 假设 release tarball 里 binary 已签好,dev 场景已被 strict-mode block)

---

## Known Risks & Future Work

1. **CSRF / 跨源攻击面**:runtime 继承当前 `CorsLayer::permissive()`,本次 endpoint 同其它 endpoint 一样对 cross-origin 开放。不引入新攻击面类别(下载 URL 硬编码指向 `CiferaTeam/gitim-releases`,攻击者无法注入代码),但整站 CSRF / data-exfil 问题需**另立 issue** 整治。TL;DR:恶意本地站点能诱导用户 runtime 升到官方 latest,不能执行任意代码。
2. **新 runtime 启动失败无自动 fallback**:若替换后新 `gitim-runtime` 因 regression 起不来,老进程已退出、端口空、前端 30s 超时。用户必须终端 `gitim runtime start` 手动恢复。当前缓解:`replace_binaries` 用 `keep_backup=true` 在异步阶段保留 `.old` 备份到 spawn 成功后才清理,所以失败时文件系统上仍有旧 binary 可用;但没自动恢复机制。Future work:A/B binary slot + fork-exec 前对子进程握手(3s 内响应 `/health` 才真退出)。
3. **前端错误态无 dismiss / retry**:error 状态下 UpdateIndicator 显示红色叹号 + 错误文字,但没有"重试"或"关闭"按钮。用户要么等 1 小时自动 re-check,要么刷新页面。v1 接受。
4. **端口重绑定 race**:老 runtime `exit(0)` 和新 runtime bind 同端口之间有小窗口。已在 `run_shell` 里加 10×100ms retry(AddrInUse → 等 100ms 重试),实践上足够。如果仍有问题,后续可以考虑 `SO_REUSEPORT`。
5. **`update_last_error` 字段无 reader**:Task 7 在 RuntimeState 加了 `update_last_error: Option<String>` 记录异步阶段错误,但 v1 前端不查询它(只等 30s 超时)。Future work:加 `GET /runtime/update-status` endpoint 让前端能拿到"为啥失败",减少"Update may have failed" 这种模糊提示。
6. **Windows 支持**:与 `install.sh` 一致,v1 not scoped。
7. **自动后台升级 / 静默升级**:明确不做。每次升级需用户在 UI 主动确认。
8. **版本比较使用 semver triple**:`parseVersion` 在前后端(gitim-updater 和 webui-v2 hook)都要求严格 `X.Y.Z` 格式,拒绝 `X.Y.Z.W` 和 pre-release 后缀。对当前 release 约定安全,但如果未来发 `v0.5.0-rc1` 之类 tag,前端不会提示升级。

---

## Plan 执行约束

- 每个 Task 独立可验证 → 单独 commit,commit message 前缀 `feat(webui-update)` 或同族
- Rust 改动每步跑 `cargo check --workspace` + 相关 `-p` 的 `cargo test`
- Frontend 改动跑 `bun run typecheck` / `bun run lint`(若存在)+ `bun run build`
- 完成所有 Task 后回 plan 文档打勾 + 跑 `cargo test --workspace` 全绿 + 手工 smoke(启动 runtime + WebUI,验证叹号逻辑)
