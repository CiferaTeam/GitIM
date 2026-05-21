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
    pub today: SaturationRatio,        // 今天 working/total
    pub last_7d: SaturationRatio,      // 过去 7 天累计
    pub by_day_30: Vec<DaySaturation>, // 最近 30 天的 by_day entries(sparkline 用)
    pub by_hour_24: Vec<HourSaturation>, // 过去 24h 的 by_hour entries(细粒度用)
}
```

跟 `usage_summary` 完全对称(usage_summary 也是 totals + 30 天 by_day)。
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
- 类型:`AgentSaturationLog` / `SaturationBucket` / `SaturationSummary` / `SaturationRatio`
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

## 待 plan-eng-review 决

- `is_working` 用 `AtomicBool` 包在哪个共享 state struct 上(AgentState 是 per-agent serialized,但 sampler 要快速读不想 deserialize)
- Sampler 跑期间 RuntimeState lock 持锁时长(不要阻塞 list endpoint)
- Agent add/burn 期间的 race:add 前 sampler tick 看不到、burn 后 saturation 文件留着不动
- Provider preflight 失败 / hermes profile create 失败这类 "agent 没真正 start" 期间不应该被 sample(还是 sample 但 always idle?)
- 测试:`AgentSaturationLog` 纯函数化 + sampler 集成测试用注入 interval
- 跟 `accumulate_usage_log` 是否复用同一个失败计数 pattern(看似该独立,err 语义不同)

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
