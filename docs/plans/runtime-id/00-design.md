# Runtime ID v1 — 设计共识

## 背景

当前 Runtime 没有任何 device / instance 级别的稳定标识:

- `~/.gitim/runtime.json` 只存 workspace 列表
- `~/.gitim/runtime.pid` 只是当前进程的 pid,每次启动都变
- `WorkspaceConfig`、`MeJson`、commit author 这些 git-tracked 的东西都是 user/handler 维度,跟"哪台机器"无关

这导致一类未来工作没有 anchor:

- 多机器之间的可选事件信道(节点之间互传 runtime-level event)
- 同设备识别 — agent 知道自己跟另一个 agent 是不是跑在同一台机器上,从而决定能不能走本地协作

v1 的目标是把这个 anchor 落下来,**不解决**任何分布式协调或 agent 注入,只确保:每台跑过 GitIM Runtime 的设备都有一个稳定的、本地持久化的 UUID,可以从 `/health` 拿到。后续工作有据可依。

## 用户决策(已收敛)

- **ID 来源:自生成 UUIDv4 持久化**(brainstorming 选项 B)— 不用平台原生 device ID,失效条件等价于 "用户主动 `rm -rf ~/.gitim/`"
- **v1 范围:本地 anchor 落地**(brainstorming 选项 A)— 不注入 agent,不进 git 同步;agent 注入用户回头配合其他机制一起做
- **文件载体:写入现有 `~/.gitim/runtime.json` 顶层** — 不开新文件
- **格式:UUIDv4 dashed lowercase**(36 字符,无前缀)
- **HTTP 暴露:扩展 `/health` 响应** — frontend 已经在 poll,顺手能拿到

## 生命周期与失效模式

| 场景 | 行为 |
|---|---|
| 第一次启动 Runtime | 生成 UUIDv4,写入 `runtime.json`,内存持有 |
| 后续启动 | 读 `runtime.json`,内存持有,不重写 |
| 文件不存在 | 等同"第一次启动" |
| `runtime_id` 字段缺失或为空 | 生成新 UUID,写回(workspaces 字段不动) |
| `runtime_id` 字段值不是合法 UUID | log warn → 重新生成覆盖 |
| 写盘失败(磁盘满 / 权限) | log warn,内存里持有当次 UUID 继续启动;下次启动重试 |
| 用户 `rm ~/.gitim/runtime.json` | 等同"第一次启动" — 同时也丢 workspace 列表,这是用户主动重置的语义 |
| 用户跨机器拷贝 `~/.gitim/runtime.json` | 两台机器拿到同一个 ID — 已知 footgun,v1 不防护(GitIM 整体没有"导出/导入设备身份"的能力,这个等同于复制 SSH key 的语义) |
| `dirs::home_dir()` 返回 None(罕见,某些容器/无 HOME 环境) | `ensure_runtime_id` 返回临时生成的 UUID,**不写盘**,log warn。runtime 继续启动。同一进程内调用稳定;重启换 ID。这跟现有 `user_config::write` 在 None 时的 noop 行为一致 |

## 改动面

### `crates/gitim-runtime/src/user_config.rs`

```rust
pub struct UserConfig {
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
}
```

新公开函数:

```rust
/// 启动时调用一次。读取 ~/.gitim/runtime.json,如果 runtime_id 缺失/空/格式
/// 错误,生成新 UUID 并写回(保留 workspaces)。返回当前有效的 runtime_id。
/// 写盘失败不返回 Err — log warn 后返回内存里的 UUID,让 runtime 继续启动。
pub fn ensure_runtime_id() -> String;

/// 测试用变体:接受显式路径。
pub fn ensure_runtime_id_at(path: &Path) -> String;
```

`upsert` / `remove` / `read` / `write` / `read_from` / `write_to` 不动语义。`UserConfig` 加 `runtime_id` 字段后,所有现有 write 路径自动保留它。

### `crates/gitim-runtime/src/http.rs`

```rust
pub struct RuntimeState {
    // ... existing fields ...
    /// Once-write at startup, read-only after. Empty string = startup hasn't
    /// populated yet (only observable in tests that build state via Default).
    pub runtime_id: String,
}

#[derive(Serialize)]
pub struct HealthResponse {
    pub service: &'static str,
    pub version: &'static str,
    pub workspaces_count: usize,
    pub runtime_id: String,  // 新增
}

async fn health(State(state): State<SharedRuntimeState>) -> Json<HealthResponse> {
    let s = state.lock().unwrap();
    Json(HealthResponse {
        service: "gitim-runtime",
        version: env!("CARGO_PKG_VERSION"),
        workspaces_count: s.workspaces.len(),
        runtime_id: s.runtime_id.clone(),
    })
}
```

`Default` impl 里 `runtime_id` 初始化为 `String::new()`,真正的赋值在 `bin/runtime.rs::run_shell()` 里。

### `crates/gitim-runtime/src/bin/runtime.rs::run_shell()`

在 `recover_from_config` **之前**调用(无依赖关系,但放前面让启动日志的"runtime started, id: …"先于 workspace recovery 输出,日志可读性更好):

```rust
let runtime_id = gitim_runtime::user_config::ensure_runtime_id();
state.lock().unwrap().runtime_id = runtime_id.clone();
eprintln!("runtime started, id: {runtime_id}");
```

## 关键不变量

1. **runtime_id 在所有 write 路径上保留**。`UserConfig` 加字段后,`upsert` / `remove` 操作 `&mut self`,字段不被触碰即保留。`write_to(cfg, path)` 整个序列化 cfg → 也保留。无需在 upsert/remove 里特殊处理。
2. **RuntimeState.runtime_id 是 once-write**。只在 `run_shell()` 启动阶段写一次,之后所有 read 都通过 `state.lock()`。不需要原子类型,锁本身已经够用。
3. **`ensure_runtime_id` 自愈语义**。失败的写盘不阻塞 runtime 启动 — 这是 daemon-style 程序的常规模式,跟现有 token_propagation / email_backfill 的"best-effort + `tracing::warn!`"一致。
4. **HealthResponse 加字段是非破坏性**。axum `Json` 序列化 + 现有 frontend 用 JSON.parse 忽略未知字段是 default。但反向 — `HealthResponse` 的 Rust struct 加新字段的同时,任何反序列化老 JSON 的测试位置都要确认 `#[serde(default)]`(这个 struct 当前只在 server 端构造、不反序列化,所以无影响,但 plan 阶段需要 grep 确认)。

## 测试覆盖

### `user_config.rs` 单元测试新增

- `ensure_runtime_id_creates_when_missing`:文件不存在 → ensure → 文件存在,字段是合法 UUID
- `ensure_runtime_id_returns_same_on_second_call`:第一次 ensure → 第二次 ensure → 同一个 UUID,文件 mtime 不变(或:用计数器验证只写一次)
- `ensure_runtime_id_regenerates_on_corruption`:写入 `runtime_id: "not-a-uuid"` → ensure → 返回新 UUID,文件被覆盖
- `ensure_runtime_id_regenerates_on_empty`:写入 `runtime_id: ""` → ensure → 返回新 UUID
- `ensure_runtime_id_preserves_workspaces`:write 含 2 个 workspace → ensure(从无 ID 状态) → workspaces 仍是 2 个
- `legacy_config_without_runtime_id_loads`:旧 schema(只有 workspaces 字段) → read → `runtime_id == ""`(serde default) → ensure 流程接管

### `http.rs` 单元测试新增

- `health_response_includes_runtime_id`:构造 `RuntimeState { runtime_id: "test-id".into(), .. }` → 调 health → JSON 含 `"runtime_id":"test-id"`

### 集成测试新增

放 `tests/runtime_id.rs`,跟 `tests/http_workspaces.rs` 同风格(直接构造 router + 调 handler,不启 socket)。比纯 unit test 多一层 `bin/runtime.rs::run_shell` 的串联验证 — `ensure_runtime_id` → 注入 state → /health 透传,这条链路任何一环错都被 unit test 漏掉:

- `health_returns_uuid`:用临时 HOME 启动一次 → 调 /health handler → `runtime_id` 字段是合法 UUIDv4
- `restart_preserves_runtime_id`:同一临时 HOME 模拟两次启动序列 → 两次 /health 返回的 ID 相同

## Non-goals(v1 明确不做)

- 不向 daemon / agent 注入(env、config、HTTP 都不传)
- 不写入 git-tracked 文件(`users/<handler>.meta.yaml`、commit trailer 都不动)
- 不设计跨机识别协议(meta.yaml 是否加 runtime_id、走 commit metadata 还是单独文件,留给 v2)
- 不实现"重置 / 轮换" CLI 命令(用户要重置就手动改文件;后续可加 `gitim runtime reset-id` 但 v1 不做)
- 不做平台原生 device ID 集成(macOS IOPlatformUUID / Linux machine-id),也不做硬件迁移检测
- 不防护"用户跨机器复制 runtime.json"(已知 footgun,接受)
- 不做 multi-runtime-instance-per-device 的支持(目前 GitIM 也不支持,假设 1 device : 1 runtime)

## v2 留下的钩子

- `RuntimeState.runtime_id` 是 process-wide single source — v2 spawn daemon 时直接 `cmd.env("GITIM_RUNTIME_ID", &state.runtime_id)` 即可
- `/health` 已暴露 — v2 frontend 想做 "多 runtime 拓扑视图" 直接用
- `runtime.json` schema 已扩展为对象 — v2 需要 device-level metadata(hostname、首次启动时间、OS、arch)直接挂同一对象顶层,旧字段不受影响
