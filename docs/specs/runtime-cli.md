# gitim-runtime CLI

> 给 agent / agent operator 的 runtime 管理命令行
> 配套 [`docs/specs/cli.md`](./cli.md)（gitim 协议命令）一起读

---

## 定位

`gitim-runtime` 是单 binary 双模式：

- **无 subcommand**（默认）→ HTTP server。listen `127.0.0.1:16868`，背后是 WebUI 和 agent 生命周期管理。这是历史行为，零回归。
- **有 subcommand** → 一次性 CLI。fork-exec 完跑完即退，本质上是本机已经在跑的 runtime HTTP API 的 thin shell wrapper。

CLI 模式是给 AI agent 用 Bash tool shell-out、给 operator 在终端临时操作设计的。它**只操作本机 runtime**：没有 `--node`、没有远程路由、没有跨节点 awareness。跨节点协作通过 GitIM 协议自身完成（agent 在同一 workspace 互发 `.thread` 消息），machine 是 invisible，agent 是 first-class。

跟人面 `gitim` CLI 的关键区别：

| | `gitim`（协议 CLI） | `gitim-runtime`（管理 CLI） |
|---|---|---|
| 后端 | per-clone daemon via Unix socket | 本机 runtime via HTTP 127.0.0.1 |
| 操作对象 | 消息 / 频道 / DM / 搜索 | agent 生命周期 / runtime 状态 / 工作区 |
| 谁用 | 人 + agent 都用 | 主要 agent，operator 调试时也用 |

设计理由见 [`docs/plans/runtime-cli/00-requirements.md`](../plans/runtime-cli/00-requirements.md)（Architecture decisions §2-§3 是为什么不复用 `gitim` 入口、为什么不引入 `gitim-runtimectl` 独立 binary 的完整说明）。

---

## 通用约定

### 输出

- **stdout**：成功输出。默认 compact JSON（一行），`status` / `runtime-id` / `preflight` 输出 pretty JSON 是因为这几个命令的消费者主要是人 + agent 读，不是 jq 管道。
- **stderr**：error envelope（`ErrorResponse` 形状，pretty 打印）+ tracing WARN 级日志。CLI 模式下 tracing 默认 WARN 不是 INFO，避免污染 stdout 的 JSON。
- **格式稳定**：输出 schema 由 CLI 自己定义的 typed wire DTOs 保证（`AgentView` / `AgentDetail` / `RuntimeStatus` / `ErrorResponse` / `AddAgentResponse`），不裸 expose runtime 内部 struct。

### Error envelope（stderr 上的失败消息）

```json
{
  "ok": false,
  "error": "runtime error [handler_conflict]: handler alice already exists",
  "error_code": "handler_conflict"
}
```

- `ok`：固定 `false`
- `error`：human-readable 消息
- `error_code`：结构化错误码（仅当 runtime 返回了 body 里有 `error_code` 字段时存在）

### Exit codes

| Code | 含义 | Agent 行为 |
|---|---|---|
| 0 | 成功 | continue |
| 1 | CLI 内部错（network 不通 / parse / config 不合法） | retry-after-fix-config |
| 2 | 服务端 permanent 错（body 含 `error_code` 或 4xx 无 `error_code`） | **不要 retry**，改输入 |
| 3 | 服务端 transient 错（5xx） | 可 retry with backoff |

**关键**：error classification 靠 **响应 body 里的 `error_code` 字段**，不靠 HTTP status 单独判断。runtime 多处 failure 路径返回 **HTTP 200 + `{ok:false, error_code:"..."}`**——这是 inherited 历史 contract，不在本 spec scope 修。CLI 把 `error_code` 当 canonical 信号，HTTP status 只在没有 `error_code` 时兜底。

### Port discovery

CLI 解析 base URL 的优先级（高 → 低）：

1. `--port <N>` flag（**仅 server 模式**，subcommand 不接 `--port`；CLI 模式当前没有 `--port`，靠下面三档发现）
2. `GITIM_RUNTIME_PORT` env var（解析为 u16，garbage 值降级到下一档）
3. `~/.gitim/runtime.json` 的 `listen_port` 字段（runtime 启动 bind 成功后写回）
4. `DEFAULT_PORT = 16868`

CLI 始终连 `http://127.0.0.1:<port>`，永远 loopback。

**注意 caveat**：`listen_port` 由 runtime 启动时 bind 成功后写入 `runtime.json`。runtime 启动 → bind → 写 listen_port 这几毫秒之间 CLI 跟丢的概率存在但极小（正常 runtime 已经在跑了，写过一次就稳定了）。本 spec 不保证该窗口期的语义，agent 重试一次即可。

### Workspace selection

workspace-scoped 命令接受 `--workspace <slug>`：

- **显式给** → 直接用，找不到则报错并列出可用 slug
- **省略** + 恰好一个 workspace → 自动用
- **省略** + 零或多个 workspace → 报错要求显式

错误形如：

```text
multiple workspaces, specify --workspace: [frontend, backend]
```

### 不在 v1 scope

- 远程路由 / 跨节点
- token / API key auth（runtime 只接受 localhost；CORS permissive 是 inherited known risk，见 `00-requirements.md` §8）
- listen_port 多实例共存
- agent 自动招募 policy（CLI 只提供 mechanism）
- SSE 订阅 / watch loop（CLI 是 one-shot；要 watch 就 caller 自己循环调）

---

## 命令清单

8 个 subcommand。下面分小节列详情。

### `gitim-runtime status`

Runtime 整体状态。聚合 `GET /health` 和 `GET /workspaces` + 每个 workspace 的 `GET /workspaces/{slug}/agents`。

**参数**：无。

**输出**（pretty JSON）：

```
$ gitim-runtime status
{
  "runtime_id": "01HABCD1234EFGH5678IJKLMNO",
  "version": "0.5.3",
  "uptime_secs": 0,
  "workspaces_count": 2,
  "agents_total": 5
}
```

**Caveat**：`uptime_secs` 当前固定为 0，runtime `/health` 没暴露 start time。Tracked v2。

---

### `gitim-runtime runtime-id`

打印本机 runtime 的 device-bound UUID（首次启动生成，落 `~/.gitim/runtime.json`，永不进 git）。

```
$ gitim-runtime runtime-id
{
  "runtime_id": "01HABCD1234EFGH5678IJKLMNO"
}
```

Agent 用它回答"我跑在哪个 runtime 上"。

---

### `gitim-runtime workspaces`

列出本机 runtime 服务的所有 workspace。passthrough `GET /workspaces`，输出原始 `workspaces` 数组（compact JSON，方便 jq 管道）。

```
$ gitim-runtime workspaces
[{"slug":"frontend","workspace_name":"Frontend","path":"/Users/x/ws/frontend","provider":"github","initialized":true},{"slug":"backend","workspace_name":"Backend","path":"/Users/x/ws/backend","provider":"local","initialized":true}]
```

每条具体字段由 runtime 控制，CLI 不做投影 —— runtime 加字段时 CLI 不需要改。

---

### `gitim-runtime list-agents [--workspace SLUG] [--detailed]`

列 workspace 下所有 agent。

**参数**：

| 参数 | 必填 | 说明 |
|---|---|---|
| `--workspace <slug>` | 仅当多 workspace | 选 workspace；单 workspace 时自动选 |
| `--detailed` | 否 | 包含 `repo_path` / `system_prompt` / `env` / `usage` 等敏感字段 |

**默认（redacted）输出**：

```
$ gitim-runtime list-agents --workspace frontend
[{"id":"alice","handler":"alice","display_name":"Alice","status":"idle","messages_processed":42,"provider":"claude","model":"claude-opus-4-7"}]
```

默认 `AgentView` 视图**有意省略**：`repo_path`（绝对路径泄露机器布局）、`system_prompt`（私有 operational instructions）、`env`（可能含 API key 类）、`session_usage` / `usage_summary`（业务遥测）、`introduction`、`error_message`、Hermes-only 字段。**这是 "safe to log" 的承诺** —— 输出可以直接贴 Slack / CI artifact 不泄密。

**`--detailed` 输出**：

```
$ gitim-runtime list-agents --workspace frontend --detailed
[{"id":"alice","handler":"alice","display_name":"Alice","status":"idle","messages_processed":42,"provider":"claude","model":"claude-opus-4-7","repo_path":"/Users/x/ws/frontend/alice","system_prompt":"You are a helpful agent.","env":{"API_KEY":"<redacted>","LOG_LEVEL":"info"}}]
```

`--detailed` 包含全部字段，但 **`env` 仍然做 secret-key redaction**：key 大写后包含 `KEY` / `TOKEN` / `SECRET` / `PASSWORD` / `API` / `AUTH` 任一子串的，value 替换为 `"<redacted>"`。所以 `--detailed` 不是 "全裸 dump"，是 "运维信息全有，secrets 不漏"。要看原始 secret 值得 SSH 上机器直接读文件，CLI 不给。

---

### `gitim-runtime add-agent --handler H --display-name N --provider P ...`

Provision 新 agent。对应 `POST /workspaces/{slug}/agents/add`。

**参数**：

| 参数 | 必填 | 说明 |
|---|---|---|
| `--handler <h>` | MUST | 小写 a-z 0-9 hyphens, 1-39 chars；runtime enforce 格式 + 唯一性 |
| `--display-name <n>` | MUST | human-readable 显示名 |
| `--provider <p>` | MUST | `claude` / `codex` / `hermes` / `opencode` / `pi` 等 |
| `--node <node-id>` | MAY | 目标 fleet node；存在 SSH tunnel 配置时 CLI 会确保 tunnel 可用后请求该 node runtime |
| `--workspace <slug>` | 仅当多 workspace | workspace 选择 |
| `--model <m>` | MAY | provider-specific model id（e.g. `claude-opus-4-7`）。**Hermes 不用这个，用下面两个** |
| `--system-prompt <text>` | MAY | inline system prompt。跟 `--system-prompt-file` 互斥 |
| `--system-prompt-file <path>` | MAY | 从文件读 system prompt（≤ 64KB） |
| `--env KEY=VALUE` | MAY | 可重复。每个 `KEY=VALUE` 添加一条 env entry，空值合法 |
| `--introduction <text>` | MAY | agent card 上的 human blurb |
| `--no-join-general` | MAY | 不自动加入 `#general` 频道（默认会加入） |
| `--llm-provider <p>` | **Hermes only** | hermes profile 的 LLM provider id（e.g. `anthropic`, `custom:foo`） |
| `--llm-model <m>` | **Hermes only** | hermes profile 的 model id |

**Hermes 注意**：Hermes provider 走 `--llm-provider` + `--llm-model`，**不是 `--model`**。runtime 会自动 `hermes profile create --clone` 出独立 profile，跑 `hermes config set` 写入 LLM 配置。详细背景见 CLAUDE.md "Hermes profile 隔离机制"小节。

**Sample**（普通 provider）：

```
$ gitim-runtime add-agent --workspace test --handler tester-x \
    --display-name "Tester X" --provider claude --model claude-opus-4-7 \
    --system-prompt "You are a test agent."
{"ok":true,"id":"tester-x"}
```

**Sample**（Hermes）：

```
$ gitim-runtime add-agent --workspace test --handler tester-y \
    --display-name "Tester Y" --provider hermes \
    --llm-provider gemini --llm-model gemini-2.0-flash-exp \
    --system-prompt "You are a test agent."
{"ok":true,"id":"tester-y"}
```

**典型错误码**：`handler_conflict`（重名）、`hermes_not_setup`（hermes 但 user 没跑 `hermes setup`）、`hermes_profile_create_failed`（profile clone 失败）。完整对照表见下方。

---

### `gitim-runtime burn-agent --id ID [--hard]`

Departures an agent. 两种语义共享一个入口：

| Flag | 实际 endpoint | 语义 |
|---|---|---|
| 默认（无 `--hard`） | `POST /workspaces/{slug}/agents/burn` | **Ritual burn**：广播 workspace-wide departure event，写 audit commit，再清 clone。正常告别用这个 |
| `--hard` | `POST /workspaces/{slug}/agents/remove`（`hard_delete: true`） | **Hard remove**：跳过 ritual，只清 local state（clone + hermes profile + in-memory 状态）。**没有 SSE broadcast，没有 audit commit**。仅在 ritual path 跑不动时用（broken daemon / missing remote / dev resets） |

**参数**：

| 参数 | 必填 | 说明 |
|---|---|---|
| `--id <id>` | MUST | agent id（实践中 = handler，但 wire shape 是 id） |
| `--node <node-id>` | MAY | 目标 fleet node；存在 SSH tunnel 配置时 CLI 会确保 tunnel 可用后请求该 node runtime |
| `--workspace <slug>` | 仅当多 workspace | |
| `--hard` | MAY | 走 hard remove 而不是 ritual burn |

**Sample**：

```
$ gitim-runtime burn-agent --workspace test --id tester-x
{"ok":true}

$ gitim-runtime burn-agent --workspace test --id tester-x --hard
{"ok":true}
```

**典型错误**：`agent_not_found` / `not_an_agent`（id 不存在或不是 agent）。先 `list-agents` 确认。

---

### `gitim-runtime fleet tunnel ...`

管理到远端 runtime 的本机 SSH tunnel，并把该 node 注册到本机 fleet observer。

`fleet tunnel up` 会启动：

```
ssh -N -L 127.0.0.1:<local-port>:<remote-host>:<remote-port> <ssh-target>
```

成功后写入 `~/.gitim/runtime.json` 的 `fleet_nodes[]`，`base_url` 为 `http://127.0.0.1:<local-port>`，tunnel pid state 写入 `~/.gitim/fleet-tunnels/<node-id>.json`，ssh 日志写入 `~/.gitim/logs/fleet-tunnel-<node-id>.log`。

**参数**：

| 命令 | 参数 | 说明 |
|---|---|---|
| `fleet tunnel up` | `--node-id <id>` | stable node id |
| `fleet tunnel up` | `--ssh-target <target>` | ssh 目标，如 `lewis@mac-mini` |
| `fleet tunnel up` | `--remote-port <port>` | 远端 runtime port |
| `fleet tunnel up` | `--remote-host <host>` | 默认 `127.0.0.1` |
| `fleet tunnel up` | `--local-port <port>` | 可省略；省略时自动选择空闲本机端口 |
| `fleet tunnel up` | `--workspace <slug>` | 可重复；传给 fleet workspace mapping |
| `fleet tunnel status` | `--node-id <id>` | 输出 tunnel pid + runtime health |
| `fleet tunnel down` | `--node-id <id>` | 停止 tunnel 并清理 pid state |

**Sample**：

```
$ gitim-runtime fleet tunnel up \
    --node-id mac-mini \
    --ssh-target lewis@mac-mini \
    --remote-port 16868 \
    --local-port 18068 \
    --workspace room
{
  "ok": true,
  "node_id": "mac-mini",
  "base_url": "http://127.0.0.1:18068",
  "tunnel_status": "up",
  "runtime_status": "healthy",
  "node": {
    "node_id": "mac-mini",
    "base_url": "http://127.0.0.1:18068",
    "workspaces": ["room"]
  }
}

$ gitim-runtime add-agent --node mac-mini --workspace valley4 \
    --handler remote-bot --display-name "Remote Bot" --provider opencode
{"ok":true,"id":"remote-bot"}

$ gitim-runtime burn-agent --node mac-mini --workspace valley4 --id remote-bot
{"ok":true}

$ gitim-runtime fleet tunnel down --node-id mac-mini
{
  "ok": true,
  "node_id": "mac-mini",
  "tunnel_status": "down",
  "stopped_pid": 12345
}
```

---

### `gitim-runtime update-agent --id ID [--system-prompt ...] [--env ...] ...`

修改已有 agent 的可编辑字段。对应 `PATCH /workspaces/{slug}/agents/{id}`。

**至少要给一个 update flag** —— 空 patch 是用户错误，CLI 客户端就拒绝，不发 HTTP。

**参数**：

| 参数 | 必填 | 说明 |
|---|---|---|
| `--id <id>` | MUST | 要改的 agent id |
| `--workspace <slug>` | 仅当多 workspace | |
| `--system-prompt <text>` | MAY | inline 替换。跟 `--system-prompt-file` 互斥 |
| `--system-prompt-file <path>` | MAY | 从文件读（≤ 64KB） |
| `--model <m>` | MAY | 替换 model id。**先 stop agent**：runtime 拒绝在 running agent 上改 model |
| `--introduction <text>` | MAY | 替换 introduction 文案 |
| `--env KEY=VALUE` | MAY | 可重复。**全量替换**，不是 merge —— 给多少条就是新 env map 的全部内容（runtime contract 如此） |
| `--dotenv-file <path>` | MAY | 写 `.env` 文件内容（≤ 64KB），落 agent clone 根目录，`chmod 0600` |

**Sample**：

```
$ gitim-runtime update-agent --workspace test --id alice \
    --system-prompt "New system prompt." --env FOO=bar
{"ok":true}
```

**Caveat**：me.json 写 + `.env` 写**是顺序而非事务**（无 WAL）。`.env` 写失败时 me.json 已经更新，客户端收到 500，靠幂等重试恢复。CLI 不做特别处理。

**v1 不支持** "clear to null"：要清字段得在 v2 加 `--clear-system-prompt` 之类。

---

### `gitim-runtime preflight <PROVIDER> [--llm-provider X --llm-model Y]`

跑 provider CLI 的真实 hello round-trip，验证 binary 存在 / 版本 / 实际能调通 LLM。对应 `GET /preflight/{provider}`（root 级，不是 workspace-scoped）。

**参数**：

| 参数 | 必填 | 说明 |
|---|---|---|
| `<PROVIDER>` | MUST | **positional** —— `claude` / `codex` / `hermes` / `opencode` / `pi` 等。runtime 维护 whitelist |
| `--llm-provider <p>` | **Hermes only** | 走 `?llm_provider=...` query param |
| `--llm-model <m>` | **Hermes only** | 走 `?llm_model=...` query param |

非 hermes 的 provider **不能**带 `--llm-provider` / `--llm-model` —— CLI 客户端直接拒绝（exit 1），不发 HTTP。

**Sample**：

```
$ gitim-runtime preflight claude
{
  "ok": true,
  "available": true,
  "version": "1.0.32 (Claude Code)",
  "hello_response": "ok"
}

$ gitim-runtime preflight hermes --llm-provider gemini --llm-model gemini-2.0-flash-exp
{
  "ok": true,
  "available": true,
  "providers": [...],
  ...
}
```

输出 shape 是 provider-specific，CLI **不 type-check**，verbatim passthrough。`available: false` 也是 exit 0（preflight 是状态查询，不是 fail-fast 探针）；只有 server 4xx / transport error 才非零退出。

---

## Exit code 详解

| Code | 触发场景 | Agent 行为 |
|---|---|---|
| 0 | 成功 | continue |
| 1 | CLI 内部错（argv 解析 / connect refused / response 不是 JSON / `--workspace` 缺失 / hermes-only flag 用在非 hermes 上 / `--env` 不是 `KEY=VALUE` 格式） | retry-after-fix-config —— 看 stderr 改 config，再调 |
| 2 | 服务端 permanent 错（body 有 `error_code`，或 4xx 无 `error_code`） | **不要 retry**，改输入 —— request 在语义上被 rejected |
| 3 | 服务端 transient 错（HTTP 5xx） | 可 retry with backoff |

**特殊**：如果 runtime 返回 HTTP 5xx 但 body 含 `error_code`，CLI 仍然 classify 为 **permanent (2)** —— `error_code` 是 canonical 信号，不在 transient retry 范围。这种情况实际不应该出现，但 belt-and-suspenders 写死了。

---

## 错误码对照表

下面列已知的 `error_code` 字段值。不穷举 —— runtime 加新错误码时 CLI 自动 passthrough，agent 收到 `error_code: "<unknown>"` 时按 permanent 错处理（exit 2）即可。

| `error_code` | 出现场景 | 建议处理 |
|---|---|---|
| `handler_conflict` | `add-agent` 时 handler 在 workspace 已存在 | 换 handler；多机 split-brain 防护，详见 CLAUDE.md "Handler 冲突防护" |
| `hermes_not_setup` | `add-agent --provider hermes` 但 user 未跑 `hermes setup` 配 default profile | 提示 user 先跑 `hermes setup`，再 retry |
| `hermes_profile_create_failed` | hermes profile clone 失败（shell-out `hermes profile create --clone` 出错） | 看 stderr message 详情；可能是 hermes 版本不兼容、磁盘满、权限问题 |
| `agent_not_found` | `burn-agent` / `update-agent` 时 agent id 不存在 | 先 `list-agents` 确认 id |
| `not_an_agent` | `burn-agent` 操作的 user 是 human 不是 agent | 改 id；human user 不能 burn |
| `invalid_token` | github mode workspace 的 PAT 无效 / 过期 | 用户更新 token（v1 需手工改 config.json，v2 加 UI） |
| `token_lacks_repo_access` | PAT 有效但无该 repo access | 重新发 token，授予对应 scope |
| `insufficient_scope` | PAT scope 不够（没 `repo` / `read:user`） | 重发 token 加 scope |
| `workspace_path_exists` | `/git/init` 时 workspace 目录已存在且非空 | 换 path 或清理目录 |
| `provision_preflight_failed` | `add-agent` 时 server preflight 失败（含 LLM auth 失败 / model 不存在 / 网络不通 / timeout / 真 LLM 拒绝） | 看 `preflight_detail` 字段：`error_kind` (not_installed / timeout / other) + `output_preview` (CLI 实际输出片段) + `error` (具体消息) 帮 debug；改 config 再试 |
| `hermes_default_profile_no_llm` | `add-agent --provider hermes` 不指定 llm 但 default profile 也无 LLM 配置 | 在 `$HERMES_HOME/config.yaml` 配 `model.default` + `model.provider`，或 add 时显式 `--llm-provider X --llm-model Y` |
| `missing_llm_provider` | `add-agent --provider hermes` 只指定 `--llm-provider` 或 `--llm-model` 一个（runtime contract: 双值或双缺，不允许半残） | 同时指定 `--llm-provider` 和 `--llm-model`，或两个都省（走 default profile 继承） |

更多 error_code 可能在 runtime `crates/gitim-runtime/src/http.rs` 里搜 `error_code: "..."` 找全。

---

## Agent shell-out 范例

Coordinator agent 在 Claude Code 的 Bash tool 里跑：

```bash
# 招一个工兵 agent
result=$(gitim-runtime add-agent \
  --workspace frontend \
  --handler builder-1 \
  --display-name "Builder 1" \
  --provider claude \
  --model claude-opus-4-7 \
  --system-prompt "You build UI components. Listen to @coordinator." 2>&1)
exit_code=$?

case $exit_code in
  0)
    agent_id=$(echo "$result" | jq -r .id)
    echo "spawned $agent_id"
    ;;
  2)
    # permanent error —— 看 error_code 决定怎么改输入
    error_code=$(echo "$result" | jq -r .error_code)
    case "$error_code" in
      handler_conflict)
        echo "handler 已存在，换名"; exit 1 ;;
      hermes_not_setup)
        echo "提示 user 跑 hermes setup"; exit 1 ;;
      *)
        echo "permanent: $result"; exit 1 ;;
    esac
    ;;
  3)
    # transient 5xx —— 隔 30s retry
    sleep 30
    # 重试...
    ;;
  *)
    # CLI 错（network / config）—— 看 stderr 改
    echo "CLI error: $result" >&2
    exit 1
    ;;
esac
```

通用 pattern：

1. 调 CLI，捕获 stdout + 退出码
2. exit 0 → 解析 stdout JSON，继续工作
3. exit 2 → 解析 stderr 的 `error_code`，按对照表决定换输入还是放弃
4. exit 3 → backoff 重试
5. exit 1 → 看 stderr 改 config，重调

**关键**：每个 subcommand 都是 one-shot，agent 自己负责 watch loop / poll 节奏。runtime 不主动推 —— v1 不出 SSE 订阅。

---

## Provisioning preflight（已落地）

> 历史：`runtime-cli` v1 ship 时**没带** add-time preflight；下一 PR `provisioning-preflight` feature 补齐了。本节描述当前实际行为，旧 v1 doc 的 "Known gap" 段已过时。

**现状**：`POST /workspaces/{slug}/agents/add` 在 `handler_conflict` 检查后、`provision_agent` 调用前，调一道 server-side preflight gate。Gate 用 add request body 的 `env` / `model` / `llm_provider` / `llm_model` 调对应 provider 的 `preflight_X_with_config`（claude/codex 支持 model override；opencode/pi 只验 connectivity；hermes 双模式 ACP / chat）。

**通过** → 走原 provision_agent 流程（git clone + daemon onboard + push + me.json + hermes profile）。
**失败** → 返 `ErrorBody { ok: false, error, error_code, preflight_detail: PreflightResult }`，**零 durable agent artifact** —— 没 commit / 没 push / 没 agent_dir / 没 state entry。

### Preflight 能 catch 的（high-confidence）
- Provider CLI 没装（→ `error_kind: not_installed`）
- LLM auth 错（PAT 过期 / API key 错）→ provider 返 auth error
- Model 名拼错 / 该 model 无 access → provider 返 model error
- Hermes default profile 没装 → `hermes_not_setup`
- Hermes default profile 装了但无 `model.default` / `model.provider` 配置 → `hermes_default_profile_no_llm`（注意：这是相对旧 detect-only preflight 的**deliberate 收紧** —— 旧 detect 只验 ACP 连通，没 LLM 也算通过；新 gate 要求 default profile 真有 LLM 配置才放行，因为没 LLM 时 agent first turn 必败，preflight 提前暴露是 feature 核心）
- Network 不通 / DNS 失败 → timeout / transport error
- Provider 服务在 add 瞬间宕机

### Preflight **不**能 catch（运行时才暴露，known limitation）
- Rate limit 在 preflight 后才达到
- Prompt-specific 行为差异（preflight 用 minimal hello prompt + tempdir cwd，agent_loop 用 repo cwd + 完整 system_prompt）
- Tool / MCP server config 错（preflight 跑 `--tools ""` 隔离工具）
- Agent 跑长任务时 context window 超限
- **opencode / pi 的 model 名拼错** —— 这两个 CLI 无 per-invocation `--model` flag，preflight 只验 connectivity + auth，不验 model 名（agent 第一 turn 才暴露）
- Post-preflight 失败路径（hermes profile clone fail / apply_model_config fail）—— 仍走现有 cleanup_agent_dir + delete_profile，**remote orphan 行为不变**（已是 hermes_profile_create_failed 的现状）

### Cost note
- claude / codex preflight 烧 **agent 配置 model** 的一个 hello token（agent 配 opus 就烧 opus 一次）
- 单次 add 一次，不是 per-turn，acceptable
- 总 add 延迟 ~10-60s（preflight ~3-15s + provision ~5-45s clone-dominated for github mode）

### Agent shell-out 时如何用 `preflight_detail`
CLI 在 `provision_preflight_failed` / `hermes_default_profile_no_llm` 等 preflight-class 错误时，stderr 会 emit 结构化 block：
```
{ JSON envelope with error_code + preflight_detail }
Preflight (claude):
  Error kind: not_installed
  Provider version: 1.2.3
  Model: claude-opus-4-7
  Output preview: <≤200 chars + … if truncated>
  Detail: <only when ≠ top-level message>
```

Agent 用 stderr regex grab "Error kind:" / "Output preview:" / "Detail:" 拿 actionable 信息；exit code 仍 2（permanent，不该 retry，改 config）。

### v2 后续
- opencode / pi 支持 model override（需调研 CLI flag 支持情况）
- Provisioning preflight cache（避免同 user 同 LLM 一次 add session 多次烧 token）
- `--skip-preflight` flag for debug

---

## 关联

- 设计来源 / "为什么这么做"：[`docs/plans/runtime-cli/00-requirements.md`](../plans/runtime-cli/00-requirements.md)
- 实现位置：`crates/gitim-runtime/src/bin/runtime.rs`（clap dispatch）+ `crates/gitim-runtime/src/cli/`（DTOs、HTTP client、subcommand handlers）
- 人面协议 CLI：[`docs/specs/cli.md`](./cli.md)
- runtime HTTP API 源：`crates/gitim-runtime/src/http.rs`
- agent runtime 生命周期：CLAUDE.md "Hermes profile 隔离机制" 小节
