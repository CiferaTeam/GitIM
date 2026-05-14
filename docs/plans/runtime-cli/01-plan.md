# gitim-runtime CLI — 实施计划 (分工版)

> 配套 `00-requirements.md` 阅读。所有 Architecture decisions §1-§8 已编号引用。
> **本文档只写分工 + 步骤 + 验证标准，不嵌实际代码**（user 偏好）。代码细节由实施 agent 在每个 task 自己决定，但必须符合本文 acceptance criteria。

**Goal**: 给 `gitim-runtime` binary 加 8 个 CLI subcommand，让 agent 用 Bash tool 能管本机 runtime 上的 agent。

**Architecture**: 单 binary 双模式 (无 subcommand = server / 有 subcommand = one-shot HTTP wrapper)。CLI ↔ `127.0.0.1:<listen_port>` HTTP。复用现有 HTTP endpoints，全部 thin wrapper。

**Tech Stack**: Rust workspace stable channel; clap 4 derive; tokio; reqwest (rustls-tls); serde_json。

---

## Phase A — 基础设施 + 退役 legacy mode (3 tasks)

### Task 1 — 加 clap 依赖 + 新 main skeleton + retire legacy positional-arg mode

**Files**:
- Modify: `crates/gitim-runtime/Cargo.toml` (依赖)
- Rewrite: `crates/gitim-runtime/src/bin/runtime.rs` (整个文件)

**Acceptance**:
- [ ] Step 1.1: `Cargo.toml` 加 `clap = { version = "4", features = ["derive"] }`，跟 `gitim-cli` 用同 minor 版本对齐
- [ ] Step 1.2: 删除 `bin/runtime.rs:46-83` 的 legacy positional-arg agent mode 分支（含 usage 行 46-53）。删除后 `provision_agent` 和 `AgentLoop` 不再被 binary 直接使用 —— 但 lib 仍然 export 它们（其他 caller 例 webui flow 在用）；只删 binary 入口的调用
- [ ] Step 1.3: 用 clap derive 重写 main：定义 `Args { #[command(subcommand)] command: Option<Command>, #[arg(long)] port: Option<u16>, #[arg(long, short)] daemon: bool }`；保留 `--version` 行为
- [ ] Step 1.4: `Command` enum 占位 8 个 subcommand variants（每个先 unimplemented! 或 todo! 留给后续 task 填）
- [ ] Step 1.5: 入口 dispatch — `match args.command { None => run_server(args.port, args.daemon), Some(cmd) => run_cli(cmd) }`；`run_cli` 入口先 stub return
- [ ] Step 1.6: `cargo build -p gitim-runtime` 通过；`cargo test -p gitim-runtime` 全绿（不引入 regression）
- [ ] Step 1.7: 手工 smoke: `cargo run --bin gitim-runtime -- --version` 输出 version 字符串后 exit 0
- [ ] Step 1.8: 手工 smoke: `cargo run --bin gitim-runtime -- old-positional-mode handler displayname` exit non-zero + clap 报 unknown subcommand
- [ ] Step 1.9: Commit — `feat(runtime): clap-based CLI scaffolding + retire legacy positional mode`

**Why Task 1 first**: 退役 legacy positional 模式是 hard prerequisite —— 不退役，clap subcommand 跟 argv 冲突（00-reqs Architecture §7）。

---

### Task 2 — Mode dispatch + check_env 移到 server-only 分支

**Files**:
- Modify: `crates/gitim-runtime/src/bin/runtime.rs`

**Acceptance**:
- [ ] Step 2.1: 把 `gitim_runtime::preflight::check_env()` 调用从 main 顶层 (`runtime.rs:28-31`) 移到 `run_server()` 函数体内
- [ ] Step 2.2: 把 `gitim_runtime::tool_path::ensure_common_tool_paths()` (`runtime.rs:23`) 也只在 server 路径跑 —— CLI 不需要它
- [ ] Step 2.3: `tracing_subscriber::fmt::init()` 在两种模式都跑（log 仍要有），但 CLI 模式默认 log level 设到 `warn`（避免污染 stdout JSON 输出）
- [ ] Step 2.4: 添加单元测试（在 `bin/runtime.rs` 内或新 `tests/cli_mode_dispatch.rs`）：用 `clap::Parser::try_parse_from` 喂 `vec!["gitim-runtime"]` → `args.command.is_none()` + 与 `args.daemon == false`；喂 `vec!["gitim-runtime", "status"]` → `args.command.is_some()`
- [ ] Step 2.5: 跑测试 `cargo test -p gitim-runtime --bin gitim-runtime` 验证 parse 正确
- [ ] Step 2.6: 手工 smoke: 安装 stub binary (cd /tmp; touch fake_gitim_daemon; chmod +x; PATH=/tmp:$PATH cargo run -- status) 预期不被 check_env 阻断（依赖 task 4 完成 status 才能完整 verify，但 parse 阶段已可见）
- [ ] Step 2.7: Commit — `refactor(runtime): move check_env to server-only branch, CLI mode skips`

**Why**: 00-reqs Architecture §6 —— CLI mode 不该被 binary version mismatch 阻断。

---

### Task 3 — `listen_port` 持久化到 `runtime.json`

**Files**:
- Modify: `crates/gitim-runtime/src/user_config.rs` (struct + ensure 函数)
- Modify: `crates/gitim-runtime/src/bin/runtime.rs` (server 启动后写回)
- Add tests in: `crates/gitim-runtime/src/user_config.rs` (`#[cfg(test)]` 模块)

**Acceptance**:
- [ ] Step 3.1: `UserConfig` struct 增加 `#[serde(default)] pub listen_port: Option<u16>`。`default = None` 兼容旧文件
- [ ] Step 3.2: 新增 `pub fn write_listen_port(port: u16) -> io::Result<()>` 函数（仿 `ensure_runtime_id` 风格），merge 写入 —— 不抹掉现有 fields
- [ ] Step 3.3: `run_server` 在 `axum::Server::bind` **成功后**调 `write_listen_port(port)`，失败 log warn 不 panic
- [ ] Step 3.4: 写 unit tests: 空文件 → 写 port → 读回；存在 runtime_id 文件 → 写 port → runtime_id 保留 + port 也存在；并发写防 race（用 `tempfile::tempdir`，符合 `tests/common/mod.rs` 隔离惯例）
- [ ] Step 3.5: Runtime 关闭时**不**清理 listen_port —— 下次启动可能 port 一致也可能不一致；CLI 应该 prefer 读 runtime.json 但 fallback default。`runtime.pid` 已是 transient marker，listen_port 同理但 best-effort 留着 OK
- [ ] Step 3.6: 跑测试 `cargo test -p gitim-runtime --lib user_config` 全绿
- [ ] Step 3.7: 手工 smoke: `cargo run --bin gitim-runtime` 启动 server，`cat ~/.gitim/runtime.json` 含 `"listen_port": 16868`；kill server，再 `cargo run --bin gitim-runtime -- --port 17000`，`cat ~/.gitim/runtime.json` 更新为 17000
- [ ] Step 3.8: Commit — `feat(runtime): persist listen_port to runtime.json`

**Why**: 00-reqs Architecture §5 —— CLI 启动时要能发现真实 port，不只是假设 16868。

---

## Phase B — CLI 模块脚手架 (2 tasks)

### Task 4 — CLI 模块 + HTTP client + workspace 选择 + 退出码映射

**Files**:
- Create: `crates/gitim-runtime/src/cli/mod.rs` (新模块)
- Create: `crates/gitim-runtime/src/cli/http.rs` (HTTP client wrapper)
- Create: `crates/gitim-runtime/src/cli/workspace.rs` (workspace 选择逻辑)
- Create: `crates/gitim-runtime/src/cli/exit_code.rs` (error_code → exit code mapping)
- Modify: `crates/gitim-runtime/src/lib.rs` (`pub mod cli;`)

**Acceptance**:
- [ ] Step 4.1: `cli::http::Client` —— wrap reqwest blocking client，constructor 接 `base_url: String`，提供 `get` / `post` / `patch` methods 返回 `Result<serde_json::Value, CliError>`。`CliError` 区分 `Transport`, `HttpStatus(u16)`, `Parse`, `ResponseErrorCode(String)`
- [ ] Step 4.2: Client 启动时 base_url 解析顺序（在新建 helper `resolve_base_url`）：
  1. CLI `--port` flag if present
  2. `GITIM_RUNTIME_PORT` env if present
  3. `~/.gitim/runtime.json` 的 `listen_port` if present
  4. fallback `127.0.0.1:16868` (DEFAULT_PORT)
- [ ] Step 4.3: `cli::workspace::resolve_workspace` —— 接 `Option<&str>` (`--workspace` flag), GET `/workspaces` 拿列表：
  - flag 有值 → 用之
  - flag 无值 + 列表长度 1 → 用唯一一个
  - flag 无值 + 列表长度 0 → `CliError::Transport("no workspace configured")`
  - flag 无值 + 列表长度 ≥2 → 报错列出 candidates 让 user 显式指定
- [ ] Step 4.4: `cli::exit_code::from_response`：
  - HTTP transport / parse fail → exit code 1
  - response body 含 `error_code` 字段（不论 HTTP status） → exit code 2
  - HTTP status 5xx 或空 body → exit code 3
  - 其它 → exit code 0
- [ ] Step 4.5: 写 unit tests: each branch of `resolve_base_url`（用 tempdir + GITIM_LOG_DIR 风格的 env override hack）；each branch of `from_response`（用 mock reqwest response 或 simple json fixture）；`resolve_workspace` 三种 branch
- [ ] Step 4.6: 跑测试 `cargo test -p gitim-runtime --lib cli`
- [ ] Step 4.7: Commit — `feat(runtime/cli): HTTP client + workspace resolver + exit code mapping`

**Why**: 00-reqs Architecture §1 + §4 + §5 —— 三块共同 surface 是所有 subcommand 公共依赖。**先建好，subcommand task 才能 thin**。

---

### Task 5 — CLI-side typed wire DTOs

**Files**:
- Create: `crates/gitim-runtime/src/cli/dto.rs`
- Modify: `crates/gitim-runtime/src/cli/mod.rs` (`pub mod dto;`)

**Acceptance**:
- [ ] Step 5.1: 定义 `AgentView`（默认 redacted）：`id`, `handler`, `display_name`, `status`, `last_activity`, `messages_processed`, `provider`, `model`，全部 `Serialize + Deserialize`
- [ ] Step 5.2: 定义 `AgentDetail`（含 `--detailed` 时 expose）：在 `AgentView` 基础上加 `repo_path`, `system_prompt`, `introduction`, `env: HashMap<String, String>`, `session_usage`, `usage_summary`, `llm_provider`, `llm_model`, `error_message`
- [ ] Step 5.3: `env` 字段 deserialize 时 pass-through，serialize 默认走 `redact_env_secrets` helper：key 含 `KEY` / `TOKEN` / `SECRET` / `PASSWORD` / `API` substring (case-insensitive) → value 替换 `"<redacted>"`；其余原样
- [ ] Step 5.4: 定义 `RuntimeStatus`：`runtime_id`, `version`, `uptime_secs`, `workspaces_count`, `agents_total`，`Serialize + Deserialize`
- [ ] Step 5.5: 定义 `ErrorResponse`：`ok: bool` (always false), `error_code: Option<String>`, `error: String`，`Serialize + Deserialize`
- [ ] Step 5.6: 定义 `AddAgentResponse`：`ok: bool`, `id: String`（match `agents_add` 现状），`Serialize + Deserialize`
- [ ] Step 5.7: 写 unit tests: 解析 runtime 真实 response JSON (用 fixture 文件 in `tests/fixtures/cli/`，从 codex review verify 过的 endpoint shape 取样); `redact_env_secrets` 测试 KEY/TOKEN/SECRET/PASSWORD 都被替换 + 普通 key 不动 + case-insensitive 工作
- [ ] Step 5.8: 跑测试 `cargo test -p gitim-runtime --lib cli::dto`
- [ ] Step 5.9: Commit — `feat(runtime/cli): typed wire DTOs with env redaction`

**Why**: 00-reqs Architecture §2 + §3 —— CLI 不用 runtime lib 的 private structs，自定义 wire DTO，含 secret redaction。

---

## Phase C — Subcommand 实施 (8 tasks)

每个 subcommand task 的 template：
1. 实现 subcommand handler function（接 clap-parsed args + Client → 输出 JSON to stdout）
2. Wire 进 main.rs 的 `Command` enum match
3. 写 Layer B integration test（复用 `tests/common/mod.rs` 起真 runtime + 直接 call handler function）
4. 验证 `cargo build` + `cargo test` 全绿
5. Commit

### Task 6 — `status` + `runtime-id` subcommand

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_status.rs`
- Create: `crates/gitim-runtime/src/cli/cmd_runtime_id.rs`
- Modify: `crates/gitim-runtime/src/cli/mod.rs` (`pub mod cmd_status; pub mod cmd_runtime_id;`)
- Modify: `crates/gitim-runtime/src/bin/runtime.rs` (wire Command match arms)
- Create: `crates/gitim-runtime/tests/cli_status.rs`

**Acceptance**:
- [ ] Step 6.1: `cmd_status::run(args, client) -> Result<i32, CliError>`：GET `/health` + GET `/workspaces` 聚合（runtime_id, version, uptime, workspaces_count, agents_total），格式化为 `RuntimeStatus` JSON 写 stdout
- [ ] Step 6.2: `cmd_runtime_id::run` —— 同 status flow，但只输出 `{"runtime_id":"..."}` 一行
- [ ] Step 6.3: 测试: 起真 runtime (复用 `tests/common::ensure_daemon_in_path`)；直接 call `cmd_status::run` w/ stub args；assert stdout JSON 含 `runtime_id` 字段且是 UUID 格式；assert exit_code = 0
- [ ] Step 6.4: 测试 runtime 关闭情况: 不起 runtime → call → exit_code = 1 + stderr 含 "cannot reach"
- [ ] Step 6.5: 测试 runtime_id endpoint 也能跑通
- [ ] Step 6.6: `cargo test -p gitim-runtime --test cli_status` 全绿
- [ ] Step 6.7: Commit — `feat(runtime/cli): status + runtime-id subcommand`

---

### Task 7 — `workspaces` subcommand

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_workspaces.rs`
- Modify: `cli/mod.rs` + `bin/runtime.rs`
- Create: `crates/gitim-runtime/tests/cli_workspaces.rs`

**Acceptance**:
- [ ] Step 7.1: handler: GET `/workspaces` → 输出 `[{slug, workspace_name, path, ...}]` JSON array
- [ ] Step 7.2: 测试: 0 workspace runtime (新启的) → 输出 `[]`；1 workspace → 输出长度 1；
- [ ] Step 7.3: 测试 `--pretty` flag 让输出 indent + 人类可读
- [ ] Step 7.4: `cargo test --test cli_workspaces` 全绿
- [ ] Step 7.5: Commit — `feat(runtime/cli): workspaces subcommand`

---

### Task 8 — `list-agents` subcommand (redacted default + --detailed flag)

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_list_agents.rs`
- Modify: wiring
- Create: `crates/gitim-runtime/tests/cli_list_agents.rs`

**Acceptance**:
- [ ] Step 8.1: handler clap args: `--workspace <slug>` (opt), `--detailed` (bool flag)
- [ ] Step 8.2: handler flow: `resolve_workspace` → GET `/workspaces/{slug}/agents` → 解析 response array → 默认 map 到 `AgentView` (redacted), `--detailed` 走 `AgentDetail` (env 仍走 redact_env_secrets) → 输出 JSON
- [ ] Step 8.3: 测试: runtime + 1 agent (mock add via existing test harness)；call `cmd_list_agents::run` 默认 → JSON 不含 `repo_path` / `system_prompt` / `env`
- [ ] Step 8.4: 测试: 同 setup w/ `--detailed=true` → JSON 含 `repo_path`, `system_prompt`, `env` 但 env 里 secret key 已被 redact
- [ ] Step 8.5: 测试: agent 的 me.json 含 `env: {API_KEY: "real-secret"}` → detailed 输出 env 显示 `"API_KEY": "<redacted>"`
- [ ] Step 8.6: 测试 workspace 解析多 ws 报错 path
- [ ] Step 8.7: `cargo test --test cli_list_agents` 全绿
- [ ] Step 8.8: Commit — `feat(runtime/cli): list-agents with redacted default + --detailed flag`

---

### Task 9 — `add-agent` subcommand (含 Hermes 特殊分支)

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_add_agent.rs`
- Modify: wiring
- Create: `crates/gitim-runtime/tests/cli_add_agent.rs`

**Acceptance**:
- [ ] Step 9.1: handler clap args: `--workspace`, `--handler` (req), `--display-name` (req), `--provider` (req), `--model` (opt), `--system-prompt` / `--system-prompt-file` (opt 互斥), `--env KEY=VAL` (repeatable), `--introduction` (opt), `--join-general` (bool, default true), `--llm-provider` / `--llm-model` (opt, Hermes only)
- [ ] Step 9.2: handler flow: assemble JSON body match `AgentAddRequest` (`http.rs:1993-2025`)；POST `/workspaces/{slug}/agents/add`；解析 response `{ok, id}` → 输出
- [ ] Step 9.3: Validation: 如果 `--provider hermes` 但缺 `--llm-provider`/`--llm-model` → 仅 warn（runtime 默认 keep cloned profile），不 error
- [ ] Step 9.4: Validation: 如果 `--provider != hermes` 但传了 `--llm-provider` → error "llm-provider only valid for hermes"
- [ ] Step 9.5: 测试: 起 runtime + 1 workspace；调 add-agent w/ claude provider → response 200 + `{ok:true, id:"..."}` + exit_code 0
- [ ] Step 9.6: 测试: 重复 add 同 handler → response 含 `error_code: "handler_conflict"` (注意 HTTP 200，靠 body classify) → exit_code 2
- [ ] Step 9.7: 测试 Hermes 路径: 起 mock runtime with hermes preflight 通过的环境（或者用 unit-level test 验证 request body 形态正确，不实跑 hermes profile create）
- [ ] Step 9.8: 测试 `--system-prompt-file` 读文件内容拼入 body
- [ ] Step 9.9: `cargo test --test cli_add_agent` 全绿
- [ ] Step 9.10: Commit — `feat(runtime/cli): add-agent subcommand with Hermes branch`

---

### Task 10 — `burn-agent` subcommand (默认 ritual + `--hard`)

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_burn_agent.rs`
- Modify: wiring
- Create: `crates/gitim-runtime/tests/cli_burn_agent.rs`

**Acceptance**:
- [ ] Step 10.1: handler clap args: `--workspace`, `--agent-id` (req, 可以叫 `--id` 简化), `--hard` (bool, default false)
- [ ] Step 10.2: handler flow:
  - `--hard=false` (默认): POST `/workspaces/{slug}/agents/burn` w/ body `{agent_id}` → 公开 ritual，触发广播
  - `--hard=true`: POST `/workspaces/{slug}/agents/remove` → hard delete 绕过公告
- [ ] Step 10.3: 测试: add 一个 agent → burn 默认 → workspace 频道有 burn 公告；list-agents 不再显示该 agent
- [ ] Step 10.4: 测试: add → burn --hard → 不见公告 + agent dir 物理清理（包括 hermes profile 如果是 hermes provider）
- [ ] Step 10.5: 测试: burn 不存在 agent → `error_code: "agent_not_found"` → exit_code 2
- [ ] Step 10.6: `cargo test --test cli_burn_agent` 全绿
- [ ] Step 10.7: Commit — `feat(runtime/cli): burn-agent subcommand with --hard option`

---

### Task 11 — `update-agent` subcommand

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_update_agent.rs`
- Modify: wiring
- Create: `crates/gitim-runtime/tests/cli_update_agent.rs`

**Acceptance**:
- [ ] Step 11.1: handler clap args: `--workspace`, `--id`, `--system-prompt` (opt), `--system-prompt-file` (opt 互斥), `--env KEY=VAL` (repeatable, replace 整个 map), `--introduction` (opt), `--dotenv-file` (path, 读文件内容上传)
- [ ] Step 11.2: handler flow: assemble PATCH body (符合 `agents_patch` handler 的 schema —— see `http.rs` `AgentPatchRequest` if exists or grep)；PATCH `/workspaces/{slug}/agents/{id}`
- [ ] Step 11.3: 处理 dotenv 文件: 64KB 上限 (符合 CLAUDE.md 提到的 `.env` 写入约束), chmod 0600
- [ ] Step 11.4: 测试: update existing agent 的 system_prompt → list-agents --detailed 显示更新后的值
- [ ] Step 11.5: 测试: update env 含 secret-key shaped key → list --detailed 显示 redacted
- [ ] Step 11.6: 测试: update 不存在 agent → exit_code 2
- [ ] Step 11.7: `cargo test --test cli_update_agent` 全绿
- [ ] Step 11.8: Commit — `feat(runtime/cli): update-agent subcommand`

**先做 Layer B verify**：如果 `agents_patch` 现有 endpoint schema 不完整支持 system_prompt/env/dotenv 同时 patch，先 fix runtime side or scope down 这个 task

---

### Task 12 — `preflight` subcommand

**Files**:
- Create: `crates/gitim-runtime/src/cli/cmd_preflight.rs`
- Modify: wiring
- Create: `crates/gitim-runtime/tests/cli_preflight.rs`

**Acceptance**:
- [ ] Step 12.1: handler clap args: `<provider>` positional (e.g. `claude` / `codex` / `hermes` / `opencode` / `pi`)
- [ ] Step 12.2: handler flow: GET `/preflight/{provider}` → parse response → 输出 `{available: bool, version: Option<String>, hello: Option<String>, error: Option<String>}` (按现有 endpoint 实际 response shape，需 grep `crates/gitim-runtime/src/preflight.rs` 或 http.rs verify)
- [ ] Step 12.3: 测试: preflight claude installed → ok response (跳过测试如果 CI 无 claude)
- [ ] Step 12.4: 测试: preflight 不存在 provider → 4xx 或 error_code → exit_code 2
- [ ] Step 12.5: `cargo test --test cli_preflight` 全绿
- [ ] Step 12.6: Commit — `feat(runtime/cli): preflight subcommand`

---

## Phase D — Test infrastructure (2 tasks)

### Task 13 — Layer A: clap argv parse unit tests

**Files**:
- Create: `crates/gitim-runtime/src/bin/runtime.rs` 内部 `#[cfg(test)] mod argv_tests` 或新 `tests/cli_argv.rs`

**Acceptance**:
- [ ] Step 13.1: 测试 `Args::try_parse_from(["gitim-runtime"])` → no subcommand + daemon=false + port=None
- [ ] Step 13.2: 测试 `["gitim-runtime", "--version"]` → 走 clap version handler (clap auto-emit)
- [ ] Step 13.3: 测试 `["gitim-runtime", "--port", "1234"]` → port=Some(1234), no subcommand
- [ ] Step 13.4: 测试 `["gitim-runtime", "--port"]` (缺 value) → parse error
- [ ] Step 13.5: 测试 `["gitim-runtime", "--port", "0"]` → port=Some(0) (clap 不 reject)
- [ ] Step 13.6: 测试 `["gitim-runtime", "--daemon", "--port", "5000"]` → 两个都 set
- [ ] Step 13.7: 测试 `["gitim-runtime", "unknown-subcommand"]` → parse error
- [ ] Step 13.8: 测试 `["gitim-runtime", "https://github.com/o/r", "handler", "name"]` (旧 legacy positional) → parse error (Task 1 已退役)
- [ ] Step 13.9: 测试每个 subcommand 的 happy parse + 缺必填 arg 的 fail parse
- [ ] Step 13.10: `cargo test -p gitim-runtime --test cli_argv` 全绿
- [ ] Step 13.11: Commit — `test(runtime/cli): clap argv edge case unit tests`

---

### Task 14 — Layer C: binary subprocess smoke test

**Files**:
- Create: `crates/gitim-runtime/tests/cli_subprocess_smoke.rs`

**Acceptance**:
- [ ] Step 14.1: 用 `Command::new(env!("CARGO_BIN_EXE_gitim-runtime"))` 起子进程
- [ ] Step 14.2: 测试 `--version` —— stdout 含 "gitim-runtime"，exit 0
- [ ] Step 14.3: 测试 no args —— spawn 子进程，sleep 500ms，curl `/health` 拿响应，kill 子进程；assert /health 响应 200 含 runtime_id
- [ ] Step 14.4: 测试 legacy positional args —— `gitim-runtime old-url handler name` exit non-zero
- [ ] Step 14.5: 测试 unknown subcommand —— exit non-zero
- [ ] Step 14.6: 用 `tests/common/mod.rs::ensure_daemon_in_path` 隔离子进程 logs
- [ ] Step 14.7: `#[serial_test::serial]` 标注（subprocess test 跟其它 daemon-spawning test 冲突）
- [ ] Step 14.8: `cargo test --test cli_subprocess_smoke -- --include-ignored` 全绿
- [ ] Step 14.9: Commit — `test(runtime/cli): subprocess smoke test for binary modes`

---

## Phase E — 文档 (3 tasks)

### Task 15 — `docs/specs/runtime-cli.md`

**Files**:
- Create: `docs/specs/runtime-cli.md`

**Acceptance**:
- [ ] Step 15.1: 文档结构: 定位 / 8 subcommand 用法表 / 参数详解 / 输出 JSON schema / Exit code 表 / 错误码对照表 / Agent shell-out 范例
- [ ] Step 15.2: 每个 subcommand 给一个真实命令行示例 + 真实输出 JSON 片段
- [ ] Step 15.3: 给 agent shell-out 的 prompt-engineering 建议（如何 reason about exit code 2 vs 3）
- [ ] Step 15.4: 链接 `docs/plans/runtime-cli/00-requirements.md` 作为 design 来源
- [ ] Step 15.5: 同步更新 `docs/specs/cli.md` 加一条 "for runtime CLI see runtime-cli.md"
- [ ] Step 15.6: Commit — `docs(specs): add runtime-cli spec`

---

### Task 16 — 更新 `CLAUDE.md` Current Orientation

**Files**:
- Modify: `CLAUDE.md` (Current Orientation section)

**Acceptance**:
- [ ] Step 16.1: 在 "**Where we are**" 列表末尾追加一句记录 runtime CLI 落地
- [ ] Step 16.2: 在 "Crate 地图" 表里给 `gitim-runtime` 行的"关键模块"补 `cli`
- [ ] Step 16.3: 更新 `## 测试` 章节加一行 `cargo test -p gitim-runtime --test cli_*` 命令示例
- [ ] Step 16.4: Commit — `docs(claude): orient on runtime CLI landing`

---

### Task 17 — 整体回归 + final integration smoke

**Files**: 无新增

**Acceptance**:
- [ ] Step 17.1: 在 worktree 跑全量 `cargo test`（按 CLAUDE.md "跑测试的节奏" —— 末尾全量 baseline）
- [ ] Step 17.2: 手工真实场景: 起 runtime → 用 CLI 加 agent → list-agents 看到它 → update system_prompt → list --detailed 看到新 prompt → burn → list 不见
- [ ] Step 17.3: Verify release pipeline 未受影响: `./release.sh --dry-run` (or whatever 现有 release dry-run command) 通过 4-target cross-compile
- [ ] Step 17.4: 不 commit (仅 verification)；汇报状态
- [ ] Step 17.5: 进 sop-dev-mode Phase 6 (code review × 2 轮)

---

## 依赖关系图

```
T1 (clap + legacy retire)
   └── T2 (mode dispatch + check_env move)
         └── T3 (listen_port persist)        ← lib 侧独立
               └── T4 (CLI scaffolding: http + workspace + exit_code)
                     └── T5 (typed wire DTOs)
                           ├── T6 (status + runtime-id)
                           ├── T7 (workspaces)
                           ├── T8 (list-agents)
                           ├── T9 (add-agent)
                           ├── T10 (burn-agent)
                           ├── T11 (update-agent)
                           └── T12 (preflight)
                                 ├── T13 (Layer A clap argv tests)
                                 └── T14 (Layer C subprocess smoke)
                                       ├── T15 (docs/specs/runtime-cli.md)
                                       ├── T16 (CLAUDE.md update)
                                       └── T17 (regression + final smoke)
```

T6-T12 (subcommand 实施) 之间**无 strict 顺序**，但因为都在同一 `cli/` 模块 + 同一 `bin/runtime.rs` Command enum，**实际上要 sequential**（merge 冲突）。可以按"难度递增" T6 → T7 → T8 → ... → T12 排，让先做的 task 帮后续 task 把 pattern 立起来。

---

## 不在本 plan 范围（已在 00-requirements.md 写明）

- 远程 `--node` route
- Tailscale 集成
- Auth / token / shared_secret
- 修 `CorsLayer::permissive()` (pre-existing inherited risk)
- 改 runtime HTTP API 的 status code（让 4xx/5xx 更有意义）—— 单独 PR 做更好
- 让 `ErrorBody` / `AgentsListResponse` / `AgentAddResponse` pub（CLI 自己 wire DTO 即可）
- Ephemeral agent 抽象
- Auto-burn / cleanup policy
- SSE / streaming subcommand

---

## Self-review checklist (writing-plans 自检)

- [x] **Spec coverage**: 00-reqs 的 8 个 subcommand × Architecture decisions §1-§8 都有 task 落地
- [x] **No placeholder**: 每个 step 有 exact 文件路径 + exact command + 验证 expected output；中文叙述代替代码块（per user 偏好）
- [x] **Type consistency**: `AgentView` / `AgentDetail` / `RuntimeStatus` 等 type 名字跨 task 一致；`Client` / `CliError` 在 T4 定义后 T6-T12 都用同名
- [x] **TDD 节奏**: 大部分 task 测试 first，validation 后再 commit（少数 thin wrapper 测试 inline）
- [x] **Bite-sized**: 每个 step 2-5 分钟可完成
- [x] **依赖排序**: T1→T2→T3 prerequisite，T4→T5 scaffolding，T6-T12 sequential subcommand，T13-T14 test infrastructure 落后做，T15-T17 docs+regression

---

## Execution handoff

**Plan 完成并 commit。两种执行方式选一：**

1. **Subagent-Driven (推荐)**：每个 task dispatch 一个 fresh subagent 实施，task 间 review。复用现有 `subagent-driven-development` skill。
2. **Inline Execution**：在当前 session 顺序执行，checkpoint 在 phase 边界。

**或者你想再 push back / 调整某些 task scope 后再开工。**
