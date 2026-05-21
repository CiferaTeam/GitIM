# Saturation Sampler — Requirements (Phase 2 output)

> Status: grill-me 完成,等 plan-eng-review 锁架构。
> Author 路径: /sop-dev-mode → Phase 2 → 进 Phase 3。

## 用户原始诉求

> 我想加个 runtime 级别的小功能。前端已经有 working 统计。想在后端加一个 5 分钟一次的打点记录,看现在的工作负载率。效果:在一天内统计本机这 20 个 agent 的工作饱和率。5 分钟打点一次。看到一个 agent 在 working,它就算一个分子;不 working 就算分母。比如长时间负载率 10%,意味着同一时段只有十分之一 agent 在工作。如果一天中只有一小时是这种状态、其他 23 小时都空着,那负载率就更低。有了这个参数我就可以研究如何提高负载率。

## 决策

### Working 信号源
`AgentState` 加 `is_working: bool`(用 `AtomicBool` 包到 per-agent 共享 state 上,避免 sampler / loop 之间 lock 竞争)。agent_loop 在 `provider.execute()` **前** 置 true,await 返回 **后** 置 false,无论成功失败(panic-safe:用 RAII guard / Drop)。Sampler tick 直接 `load(Ordering::Relaxed)`。

- 物理对齐"现在 LLM 在跑",不是"loop 在 sleep/poll"
- 跟现有 `accumulate_usage_log` 写时机同插入点(agent_loop.rs:365)
- 不引入新 cache,不复刻前端 SSE 衍生逻辑

语义跟前端 `agentWorkState()` 略紧(前端把 tool_use → thinking 之间的瞬间也算 working,后端只算 provider.await 期间)。在 5min 采样间隔下毫秒级语义差被淹没,long-run average unbiased。

### 磁盘 Schema
Mirror `usage_log` pattern:per-agent JSON,atomic rewrite,chmod 0600。

路径:`<workspace>/.gitim-runtime/saturation/<handler>.json`

```json
{
  "version": 1,
  "handler": "alice",
  "first_seen": "2026-05-21T08:00:00Z",
  "last_updated": "2026-05-21T13:00:00Z",
  "totals": { "working_samples": 12, "total_samples": 288 },
  "by_day": {
    "2026-05-21": { "working_samples": 12, "total_samples": 288 }
  },
  "by_hour": {
    "2026-05-21T08": { "working_samples": 3, "total_samples": 12 },
    "2026-05-21T09": { "working_samples": 0, "total_samples": 12 },
    "2026-05-21T10": { "working_samples": 5, "total_samples": 12 }
  }
}
```

- `by_hour` key 用 `YYYY-MM-DDTHH`(UTC)
- 每 5min sampler tick 给当前 agent +1 到 `total_samples`(by_day / by_hour / totals 三处),`is_working == true` 时同步 +1 到 `working_samples`
- 写盘:每 tick 落一次(参考 usage_log 每 turn 落一次),atomic rename + chmod 0600
- 失败:warn-log + `RuntimeState.saturation_save_failures` AtomicU64 +1,不阻塞 sampler

### 保留期
**90 天**,跟 usage_log 一致。`prune_by_day` + `prune_by_hour` 在每次 save 时调用,drop 超过 90 天的 key。
体量估算:`24 buckets/day × 90 days × 20 agents ≈ 43200 entries ≈ 1.5MB total`。

### HTTP 暴露
`AgentInfo` 加字段:`saturation_summary: Option<SaturationSummary>`。

```rust
pub struct SaturationSummary {
    pub first_seen: String,
    pub last_updated: String,
    pub totals: SaturationBucket,           // 生命周期累计
    pub today: SaturationBucket,            // 今天 working/total
    pub last_7_days: Vec<DaySaturation>,    // 过去 7 天 by_day(含零填充)
    pub last_24_hours: Vec<HourSaturation>, // 过去 24h by_hour(含零填充)
    pub by_day_30: Vec<DaySaturation>,      // 最近 30 天 by_day(sparkline 用)
}
```

Wire shape 以 `01-plan.md` Task 1 `SaturationSummary` / `SaturationBucket` / `DaySaturation` / `HourSaturation` 为 SoT。
跟 `usage_summary` 对称(usage_summary 也是 totals + 30 天 by_day);比 usage 多 `last_7_days` / `last_24_hours` 两个零填充 projection。
前端 reduce 多个 agent 算 fleet aggregate(沿用 `summarizeAgentWorkload` pattern)。

`/runtime/health` 加 `saturation_save_failures: u64`,跟 `usage_save_failures` 并列。

### 前端 v1 UI
扩 `WorkspaceUsageHeader`:加一格 "Today saturation: X.X% (Y working / Z agents · 12-sample resolution)" + 7-day sparkline。

- 复用 `lib/sparkline.ts`
- 没有独立页面,没有 heatmap
- 仅 fleet aggregate,不在 per-agent card 上加(per-agent 数据在 detail 页 v2 加)

### Sampler 生命周期
- `RuntimeState::new()` 之后 / HTTP serve 之前 `tokio::spawn` 一个 saturation_sampler task
- 持 `Arc<Mutex<RuntimeState>>`(or `Weak` 看 plan-eng-review 决定)
- `tokio::time::interval(Duration::from_secs(300))` 主循环
- runtime 进程退出即结束,不显式 cancellation
- panic-safe:tick body 用 `tokio::spawn` 包一层或者 `catch_unwind`,单 tick 失败不杀整个 sampler
- 测试:`SAMPLING_INTERVAL` 不要 hardcode,通过 `RuntimeState` 或 builder 参数注入;集成测试用短 interval(e.g. 100ms)

### 命名
- 模块:`saturation_log.rs`(per-agent disk store)+ `saturation_sampler.rs`(background task)
- 类型:`AgentSaturationLog` / `SaturationBucket` / `SaturationSummary` / `DaySaturation` / `HourSaturation`
- 字段:`saturation_summary` / `saturation_save_failures`
- 路径:`<workspace>/.gitim-runtime/saturation/<handler>.json`

## Non-goals (v1)

- ❌ Raw 5min sample 落盘(只 by_day + by_hour 聚合,schema forward-compat 留 `by_tick` 字段位)
- ❌ SSE event 推 saturation 更新(低频指标,polling 足够)
- ❌ Fleet-wide `/runtime/saturation` endpoint(前端 reduce)
- ❌ Cross-workspace 聚合
- ❌ saturation 数据进 git 同步(per-machine local)
- ❌ Hour heatmap / 独立分析页面
- ❌ Agent hard delete 时迁移历史(直接 rm 文件,跟 usage_log 同 pattern)
- ❌ Soft delete 时动 saturation 数据
- ❌ Per-provider / per-channel 切片
- ❌ Working "duration" 直方图

## Phase 3 plan-eng-review 锁定的架构决策

### 硬约束(来自 explore)
- `RuntimeState` 是 `Arc<Mutex<...>>` 用 `std::sync::Mutex`([http.rs:430](../../../crates/gitim-runtime/src/http.rs:430)),不是 tokio mutex。任何持锁期间会阻塞 tokio worker。决定了 sampler 必须 "snapshot → drop lock → IO"。
- `AgentInfo` 已经有 `#[serde(skip)]` runtime-only 字段(`loop_handle`)。`is_working` 加进 `AgentInfo` 复用此 pattern。
- `AgentLoop` 已持 `runtime_state: Option<SharedRuntimeState>`([http.rs:63](../../../crates/gitim-runtime/src/http.rs:63))。toggle 路径已通。
- `provider.execute()` 是 `?` bubble 设计([agent_loop.rs:798-802](../../../crates/gitim-runtime/src/agent_loop.rs:798))。手写 set/reset 在 error 路径会漏 reset。
- `cleanup_agent_dir` 只 `remove_dir_all(agent_dir)`([http.rs:3146](../../../crates/gitim-runtime/src/http.rs:3146)),saturation 文件在 `<workspace>/.gitim-runtime/saturation/`,**不在 agent_dir 下**,必须显式追加清理。

### 决策

| ID | 决策 | 理由 |
|----|------|------|
| A1 | `is_working: Arc<AtomicBool>` 加到 `AgentInfo`,`#[serde(skip)]` | 跟 `loop_handle` 同 lifetime;lock-free 读写;sampler 持锁期间只 `Arc::clone` 而非 atomic load |
| A2 | Sampler tick: 持锁→snapshot `Vec<(workspace_slug, handler, Arc<AtomicBool>)>`→drop lock→atomic load + 写盘 | sync mutex 下唯一非阻塞写法。N=20 时 snapshot ≈ μs |
| A3 | provider.execute toggle 用 **RAII `WorkingGuard`**,Drop 时 store(false) | panic-safe + error-safe;数据准确性是 sampler 的 truth source,飘 false 永久污染数据 |
| A4 | Agent add/burn 期间 race 接受 | 5min 一次,最多漏一个 sample;burn 时 saturation 文件留到 hard_delete 才清 |
| C1 | v1 `saturation_log.rs` 跟 `usage_log.rs` 复制 4 个共性函数(save / load / prune / chmod_0600);不抽 trait | YAGNI;两个 struct < 300 行;第三个 metric log 出现时再抽 |
| C2 | `hard_delete_agent_dir` 后追加 `std::fs::remove_file(<workspace>/.gitim-runtime/saturation/<handler>.json)`,best-effort 失败 warn 不阻塞 | design doc 既定要求,自动 lifecycle 一致 |
| T1 | `SaturationSampler` 拆 `take_snapshot(state)` 纯函数 + `tick_once(snapshot)` 执行 IO,分别测试 | 纯函数 unit test 覆盖所有 sampling 逻辑;集成测试只验 wiring |
| T2 | Sampler interval 通过 `SaturationSampler::with_interval(d: Duration)` builder 注入,production 默认 `Duration::from_secs(300)` | 集成测试用 100ms tick 几个 tick 即可验证落盘 |
| F1 | `RuntimeState.saturation_save_failures: AtomicU64` 独立于 `usage_save_failures` | 两个 IO 路径独立,失败语义不同(saturation 失败 ≠ usage 失败) |

### Open(由 Phase 4 plan 决定 implementation 细节)
- `SaturationSampler` spawn 在 RuntimeState 构造后哪里(`run` 函数 startup 段)
- `WorkingGuard` 类型放哪个 module(`agent_loop.rs` 文件内 private 还是 saturation 子模块)
- `take_snapshot` 函数签名(借 lock guard 还是接 `&RuntimeState`)

### Tests 计划(Phase 4 plan 落地具体测试)

```
COVERAGE MAP
============================================================================
[+] crates/gitim-runtime/src/saturation_log.rs (new)
  ├── AgentSaturationLog::accumulate(working: bool, ts: DateTime)
  │   ├── [ ] new file -> first_seen + totals + by_day + by_hour 全部 +1
  │   ├── [ ] same day 累加 by_day; same hour 累加 by_hour; totals 同步
  │   ├── [ ] working=false 只 +1 total_samples 不增 working_samples
  │   └── [ ] 跨午夜 / 跨整点 创建新 bucket key
  ├── save(workspace_root, today) + load_or_default
  │   ├── [ ] atomic rename + chmod 0600 (mirror usage_log::save)
  │   └── [ ] roundtrip: save then load_or_default ≡ identity
  └── prune_by_day / prune_by_hour
      ├── [ ] drop entries 超过 RETENTION_DAYS(90)
      └── [ ] by_hour key 比较用 UTC 解析

[+] crates/gitim-runtime/src/saturation_sampler.rs (new)
  ├── take_snapshot(&RuntimeState) -> Vec<(slug, handler, working: bool)>
  │   ├── [ ] 空 workspaces -> empty vec
  │   ├── [ ] 多 workspace × 多 agent 全部枚举
  │   └── [ ] is_working atomic.load 反映当前状态
  └── tick_once(snapshot, now) (integration)
      └── [ ] 注入 100ms interval, 3 tick 后 saturation/<h>.json 内容正确

[+] crates/gitim-runtime/src/agent_loop.rs (modify)
  ├── WorkingGuard Drop -> store(false)
  │   ├── [ ] 正常 Ok 路径 reset
  │   ├── [ ] ? bubble Err 路径 reset
  │   └── [ ] 模拟 panic 时 reset (catch_unwind 包测试)
  └── set_runtime_state 注入后 is_working 字段在 RuntimeState 可见

[+] crates/gitim-runtime/src/http.rs (modify)
  ├── AgentInfo.saturation_summary 字段
  │   └── [ ] list endpoint 返回每 agent 带 summary
  ├── HealthResponse.saturation_save_failures
  │   └── [ ] /runtime/health 暴露
  └── hard_delete_agent_dir 追加清 saturation/<h>.json
      ├── [ ] hard delete 后文件不存在
      └── [ ] saturation 文件不存在时 remove_file 失败仅 warn

[+] products/gitim/frontend/src/lib/types.ts (modify)
  └── SaturationSummary type
      └── [ ] type 跟后端 wire format 一致

[+] products/gitim/frontend/src/components/management/workspace-usage-header.tsx (modify)
  └── 加 "Today saturation X.X%" + 7-day sparkline
      └── [ ] 多 agent reduce 出 fleet ratio (按 Σworking / Σtotal)

COVERAGE: 16 test entries listed here (high-level); 01-plan.md Task 1–6 总计 23 个新测试(以 plan 为准)
```

## 验收

数据层:
- [ ] 5min 跑一次,每 agent 一条 sample
- [ ] `is_working == true` 时计入分子,`false` 时只计分母
- [ ] 文件落盘:atomic rename + chmod 0600
- [ ] 写盘失败 +1 到 `saturation_save_failures`,不阻塞 sampler
- [ ] 90 天 rotation 生效(by_day 和 by_hour 都 prune)
- [ ] agent hard delete 删 saturation 文件

HTTP 层:
- [ ] `GET /workspaces/<slug>/agents` 返回每个 agent 带 saturation_summary
- [ ] `GET /runtime/health` 暴露 `saturation_save_failures`

前端层:
- [ ] `WorkspaceUsageHeader` 多一格 saturation
- [ ] 7 天 sparkline 渲染正常
- [ ] 多 agent 的 reduce 算出正确 fleet ratio

测试:
- [ ] `saturation_log` 单元测试覆盖 accumulate / prune / save / load
- [ ] sampler 集成测试:short interval(< 1s) + mock agent + 验证文件内容
- [ ] full `cargo test -p gitim-runtime` 通过
