# gitim-index Opt-in 化

**Related history**: [2026-03-23-sqlite-index.md](2026-03-23-sqlite-index.md)（原 crate 引入设计）

---

## Where we are

`gitim-index` 是为消息全文搜索建的 SQLite FTS5 crate，自引入以来:

- 唯一活跃消费者: CLI `gitim search` / `gitim reindex`（admin 命令）
- Runtime HTTP 不暴露、`products/gitim/frontend` 主线 Web 端不调用、老 `webui` 原型也无引用
- 但每个 daemon 启动都会 `initialize_index`(`crates/gitim-daemon/src/state.rs:147`)，每次 sync cycle 都会 `append_from_diff` / `rebuild`(`state.rs:313-358`)
- Workspace 模式下每个 agent 都是独立 daemon → **N 个 agent = N 份 `.gitim/index.db` + N 份后台 indexer**

结果是一个**对人类隐藏、对 agent 烧 CPU** 的索引层。

最近几次大版本（GitHub workspace / Hermes profile / token usage / cron）均未触及此 crate。功能层已冻结，只跟随消息类型做最小适配（card / DM filter）。

## Where we're going

中长期判断: 全文搜索是**人类使用** WebUI 时才需要的能力（Agent 直接 grep `.thread` 文件更高效）。未来形态可能是 WASM runtime + human clone 内的索引，不是 per-daemon。

但 WASM runtime 还是愿景、时间点不明。**当前清理只做"立刻止血"那一档**，不提前迁移到 runtime / WASM。

## Decision

加 per-clone 配置开关 `indexer.enabled`，**默认 false**:

- Agent clone: serde default 自然落到 false → daemon 启动跳过 indexer 初始化 → sync cycle 不再维护索引
- Human clone: onboard 路径显式写 `true` → 行为与今日完全一致

**保留** crate、IPC、CLI 命令、responses 类型 —— 不破坏未来重用空间。

### 为什么不选其他路径

- **位置先对齐**（把 indexer 搬到 runtime）: 几百行 plumbing，赌一个 WASM 时间点不明的未来；当前 ROI 不足
- **直接砍掉**（删 crate + 所有 hook）: 980 行 lib 里真正难写的是 DM 可见性 + 卡片过滤 + FTS5 query escape（见 `51fa698 fix(index): card + current_user 搜索不再被 DM filter 误杀`），删了重写代价不止 980 行

## Scope

### In

- `gitim_core::types::config::Config` 加 `IndexerConfig` 子结构
- `gitim-daemon` 启动分支 + `initialize_index` 早 return
- `gitim-cli` onboard 命令显式写 `enabled=true`
- `gitim-runtime` `/git/init` human 路径显式写 `enabled=true`
- `handlers/search.rs` / `handlers/reindex` 错误文案改进
- 测试: config 兼容性 + daemon 启动 disabled 路径 + 错误文案

### Out（Non-goals v1）

- 不动 `gitim-index` crate 内部（schema / 卡片过滤 / DM 可见性 / FTS5 逻辑）
- 不动 daemon IPC `Request::Search` / `Request::Reindex` 协议
- 不动 CLI `search` / `reindex` 命令存在性
- 不暴露 indexer toggle 到 WebUI / runtime HTTP
- 不做 hot-reload —— 改 config.yaml 需重启 daemon（与现有 `sync_interval` / `debug_http` 一致）
- 不做旧 `.gitim/index.db` 文件自动清理（用户后续若重新启用可复用）
- 不做旧 human clone 自动迁移（CHANGELOG 提示即可）
- 不做 `gitim reindex` 自动启用 indexer —— config.yaml 是 single source of truth

## Design

### Data shape

`Config` 加 `#[serde(default)] pub indexer: IndexerConfig`。`IndexerConfig` 单字段 `enabled: bool`，`#[derive(Default)]` 自动 false。

旧 `.gitim/config.yaml` 不带 indexer 字段也能解析（serde default 兜底），这是迁移零摩擦的核心机制。

新 default config 序列化后形如:

```yaml
version: 1
endpoint: github
endpoint_url: ""
daemon:
  sync_interval: 3
  debug_http: false
  debug_port: 3000
indexer:
  enabled: false
```

`IndexerConfig` 故意保留子结构（而不是 flat `indexer_enabled: bool`），为未来扩展（`db_path`、`include_dms`、`rebuild_on_start` 等）留空间。但 v1 只有 `enabled` 一个字段。

### Daemon 启动分支

`gitim-daemon/src/main.rs` 读完 config 后把 `config.indexer.enabled` 传给 `state::initialize_index(&state, enabled)`。

`state::initialize_index` 加 early-return: enabled=false 时直接返回 Ok，不打开 SQLite、`state.index` 保持 `RwLock<Option<Arc<Index>>>` 的初始 `None`。

**关键复用**: 现有代码已经处理 `index = None`:

- `sync_loop` 回调(`state.rs:313-319`): `match` None 直接跳过
- `handle_search`(`handlers/search.rs:17-20`): None → `Response::error`
- `handle_reindex`(`handlers/search.rs:66-71`): None → `Response::error`

因此 sync_loop body / handler body **零改动**，只在 startup 一处分支即可关停整条链路。

### Config 写入点（human 显式 true）

两条 human onboard 路径都要写:

1. **CLI `gitim onboard`** —— `crates/gitim-cli/src/commands/onboard.rs` 已有 `ensure_config_debug_http(repo_dir, enabled)` 用 regex 在 config.yaml patch 单字段，作为参考模板写 `ensure_config_indexer_enabled(repo_dir, true)`
2. **Runtime `/git/init`** —— `crates/gitim-runtime/src/onboard::provision_human` 在 me.json 写完后，对 human clone 的 `.gitim/config.yaml` 做同样 patch

**Agent 路径零改动** —— `provision_agent` 不需要写 config，daemon 启动若 config 不存在会写 default（enabled=false 即所需值）。

### 错误文案

`handlers/search.rs` 中 `handle_search` 和 `handle_reindex` 两个 handler 在 `state.index == None` 时：

- 当前两处都返回 `Response::error("search index not available")`
- 统一改为 `Response::error("search index disabled for this clone (set indexer.enabled=true in .gitim/config.yaml and restart daemon)")`

CLI `gitim search` / `gitim reindex` 在 agent clone 上失败时用户能看到可操作信息，不需要查源码。

### 迁移行为

| Clone 类型 | 升级前 | 升级后 | 用户感知 |
|---|---|---|---|
| Agent（无 indexer 字段） | 跑 indexer | serde default → false → 跳过 | sync cycle 后 `.gitim/index.db` 不再增长；旧 `.db` 文件残留但不再被读 |
| Human（无 indexer 字段，假设有人在用 CLI search） | 跑 indexer | serde default → false → 跳过 | **breaking**: `gitim search` 返回 disabled error。需手动在 config.yaml 加 `indexer.enabled: true` 重启 daemon |
| 新 Human（升级后 onboard） | n/a | onboard 写 `enabled=true` | 行为与升级前一致 |
| 新 Agent（升级后 onboard） | n/a | default false | sync 不跑 indexer |

Breaking 影响范围实测为零（前端不接、runtime 不暴露），但 CHANGELOG 明确提示，避免少数手动用 `gitim search` 的开发者困惑。

旧 `.gitim/index.db` 残留文件**不主动删**:

- 文件本身不大、不在 git 里、用户重新 `enabled=true` 时 `initialize_index` 会自动 incremental update 复用
- 删除是破坏性动作，没有正当理由触发

## Testing

- **`gitim-core::types::config`**:
  - `indexer_defaults_to_disabled` —— `Config::default().indexer.enabled == false`
  - `legacy_yaml_without_indexer_field_parses` —— 反序列化不带 `indexer:` 的旧 yaml，验证 default false
  - Roundtrip test 包含 indexer 字段
- **`gitim-daemon` 集成测试**:
  - daemon 启动 with `indexer.enabled=false` → `state.index` 为 None
  - sync cycle 跑完 → `.gitim/index.db` 文件不存在
  - `Request::Search` 返回新文案（assert error message 含 "disabled" 关键词）
  - `Request::Reindex` 返回 disabled error
- **CLI / Runtime onboard 路径**:
  - CLI `gitim onboard` 后 `.gitim/config.yaml` 含 `indexer.enabled: true`
  - Runtime `/git/init` (human) 后 human clone config 含 `indexer.enabled: true`
  - Runtime `add_agent` 后 agent clone config 不显式 set indexer（依赖 daemon default）→ 启动后 `enabled=false`
- **现有 indexer=on 路径所有测试不动**，但需要审计: 任何依赖"daemon 启动后 index 自动存在"的测试要么在 setup 里显式写 `enabled=true`、要么改成测试 None 行为

## Implementation order suggestion

按"自底向上、每步独立可 merge":

1. `gitim-core::types::config` 加 `IndexerConfig` + 测试
2. `gitim-daemon` main.rs / state.rs early-return + 集成测试
3. `handlers/search.rs` 错误文案 + 测试
4. `gitim-cli onboard` 写 indexer flag + 测试
5. `gitim-runtime /git/init` 写 indexer flag + 测试
6. CHANGELOG 更新

每步可独立提交、独立验证，回滚成本低。

## Open questions

无（设计阶段所有开放问题已在 Section 1-3 对话中决议）。
