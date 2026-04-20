# Usage Indicator — Design

**Date:** 2026-04-20
**Scope:** `products/cell/backend/`、`webui-v2/`
**Status:** Design approved via brainstorming, awaiting spec review

## Goal

在 webui-v2 顶栏右侧加一个小型活跃用户指示器，给用户(包括 cell.gitim.io 访客)一个"社区还活着"的实时信号：

- 显示今日 DAU 数字 + 30 天 sparkline 趋势
- Hover 展开放大视图
- 现有 `visitors` schema 补一张日志表，拿到真 DAU（而非 `last_seen` 按最后一次出现分桶产生的伪 DAU）

## Non-goals

- 不做登录/身份分析（纯匿名 UUID 聚合）
- 不做 polling：日粒度数据，mount 时拉一次即可
- 不做留存曲线、访问频次分布、地域分布等深度分析 —— admin endpoint 不长成 dashboard
- 不做 i18n；中文 UI 文案写死
- 不做 loading skeleton / 空态分支：请求失败整个组件隐身，真实数据是 0 就诚实画 0
- 不做"人数激增"高亮 / 通知

## 架构决策 Top-level

| 决策 | 选择 | 否决项 |
|------|------|--------|
| DAU 指标如何算 | 新建 `visits(uuid, day)` 日志表，写时 upsert | ① 用 `last_seen` 聚合（按最后一次分桶，严重低估老用户）② KV + Cron trigger 物化（复杂度高于加表） |
| 公开端点路径 | 新增 `GET /api/stats`（无鉴权） | 解除 `/admin/stats` 鉴权（路径语义混乱） |
| `/admin/stats` 扩展 | 向后**不**兼容，改成中文 key | 保留英文 + 加新字段（用户只自己看，中文方便） |
| 前端图表库 | 纯 inline SVG `<path>` | 引入 recharts / chart.js（30 点 sparkline 不值当） |
| Hover 交互组件 | Radix HoverCard（通过 `radix-ui` umbrella 包接入） | 点击触发 Popover（hover 更轻，符合"信号"定位） |
| 刷新节奏 | mount 一次，不 poll | 定时轮询 |

---

## 1. 数据层

### 新 migration `0002_create_visits.sql`

```sql
CREATE TABLE IF NOT EXISTS visits (
  uuid TEXT NOT NULL,
  day  TEXT NOT NULL,         -- 'YYYY-MM-DD' (UTC，与 /admin/stats 现有口径一致)
  PRIMARY KEY (uuid, day)
);
CREATE INDEX IF NOT EXISTS idx_visits_day ON visits(day);
```

**容量估算**：DAU × 30 天。DAU 1000 也就 ~30k 行，D1 免费档 5GB 额度无感。长远可加保留期裁剪（`DELETE WHERE day < DATE('now','-90 days')`），但 v1 不做。

### 写入路径改动

文件：`products/cell/backend/src/version.ts` 的 `recordVisitor` 函数。

在现有 `visitors` upsert 之后追加一句：

```ts
await db
  .prepare(`INSERT OR IGNORE INTO visits (uuid, day) VALUES (?1, DATE(?2))`)
  .bind(uuid, now)
  .run();
```

- `INSERT OR IGNORE` 保证同 UUID 同一天只一条
- `DATE(?2)` 用 UTC 截日，和 `/admin/stats` 现有日期口径一致
- 两条写入都在 `recordVisitor` 内顺序 await，整体仍由 caller 的 `c.executionCtx.waitUntil(...)` 包裹 fire-and-forget

---

## 2. 公开端点 `GET /api/stats`

### 新文件 `products/cell/backend/src/stats.ts`

```ts
import { Hono } from "hono";
import type { Bindings } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.get("/api/stats", async (c) => {
  const today = new Date();
  today.setUTCHours(0, 0, 0, 0);
  const startMs = today.getTime() - 29 * 86400_000;
  const startDay = new Date(startMs).toISOString().slice(0, 10);

  const rows = await c.env.CELL_DB
    .prepare(
      `SELECT day, COUNT(*) AS dau
       FROM visits
       WHERE day >= ?1
       GROUP BY day`,
    )
    .bind(startDay)
    .all<{ day: string; dau: number }>();

  const byDay = new Map<string, number>();
  for (const r of rows.results ?? []) byDay.set(r.day, r.dau);

  const days: { date: string; dau: number }[] = [];
  for (let i = 0; i < 30; i++) {
    const date = new Date(startMs + i * 86400_000).toISOString().slice(0, 10);
    days.push({ date, dau: byDay.get(date) ?? 0 });
  }

  c.header("Cache-Control", "public, max-age=60");
  return c.json({ days });
});

export { app as statsRoutes };
```

### 挂载

`products/cell/backend/src/index.ts`：`app.route("/", statsRoutes)`。

### CORS

已有 cors middleware 覆盖所有路径。`origin` 白名单：
- `localhost:*`（本地 runtime WebUI）
- `cell.gitim.io`
- `*.cell-gitim.pages.dev`

新端点自动享受。无需改动。

### Response Shape（英文 key，程序契约）

```json
{
  "days": [
    { "date": "2026-03-22", "dau": 0 },
    ...（共 30 项，最后一项是今天）
    { "date": "2026-04-20", "dau": 12 }
  ]
}
```

---

## 3. `/admin/stats` 扩展（中文 key）

**破坏性变更**：原英文 shape 废弃。唯一消费者是用户本人（已确认）。

### 查询

```ts
// days[]: 聚合 new_uv / returning_uv / dau / cumulative
// 一条复合查询即可（LEFT JOIN visits + visitors），或两次独立查询在内存里 merge：
//   Q1: SELECT DATE(first_seen), COUNT(*) FROM visitors WHERE first_seen >= startIso GROUP BY ...
//   Q2: SELECT day, COUNT(*) FROM visits WHERE day >= startDay GROUP BY day
//   merge: returning_uv = dau - new_uv; cumulative = running sum of new_uv

// summary
// active_today:   SELECT COUNT(*) FROM visits WHERE day = today
// active_7d:      SELECT COUNT(DISTINCT uuid) FROM visits WHERE day >= today - 6d
// active_30d:     SELECT COUNT(DISTINCT uuid) FROM visits WHERE day >= today - 29d
// total_uv:       SELECT COUNT(*) FROM visitors
// stickiness_30d: avg(days[].dau) / active_30d   // DAU 均值 / MAU，行业通用粘性
```

### Response Shape

```json
{
  "每日": [
    {
      "日期": "2026-04-20",
      "新增": 3,
      "回访": 9,
      "日活": 12,
      "累计新增": 456
    }
  ],
  "汇总": {
    "今日活跃": 12,
    "近7天活跃": 48,
    "近30天活跃": 156,
    "累计用户": 456,
    "30天粘性": 0.12
  }
}
```

字段说明：

- `新增` = `new_uv`，当天 `first_seen` 的人数
- `回访` = `dau - new_uv`，当天活跃里不是新人的
- `日活` = `dau`，当天 distinct uuid（来自 `visits`）
- `累计新增` = running sum of `新增`，30 天窗口内的成长曲线
- `30天粘性` = 30 天每日 DAU 的平均值 / 30 天 MAU，0-1 小数，两位精度

---

## 4. 前端组件 `UsageIndicator`

### 新增文件

| 路径 | 职责 |
|------|------|
| `webui-v2/src/lib/cell-api.ts` (extend) | 加 `fetchStats()` |
| `webui-v2/src/hooks/use-stats.ts` | mount 时拉一次，失败返回 null |
| `webui-v2/src/components/ui/hover-card.tsx` | Radix HoverCard 封装（项目现无此组件） |
| `webui-v2/src/components/usage-indicator.tsx` | 顶栏组件 |

### `fetchStats`

```ts
interface StatsResult {
  days: { date: string; dau: number }[];
}

export async function fetchStats(): Promise<StatsResult | null> {
  try {
    const res = await fetch(`${API_URL}/api/stats`);
    if (!res.ok) return null;
    return (await res.json()) as StatsResult;
  } catch {
    return null;
  }
}
```

### `useStats`

```ts
export function useStats() {
  const [days, setDays] = useState<{ date: string; dau: number }[] | null>(null);
  useEffect(() => {
    fetchStats().then((r) => r && setDays(r.days));
  }, []);
  return days;
}
```

失败时 `days` 保持 `null`，组件不渲染。

### `UsageIndicator` 组件

```
┌──────────┐
│ 📈 12    │  ← 顶栏 inline trigger，约 60px 宽
└──────────┘
   ↓ hover
┌────────────────────────────────┐
│ 12 人正在使用 GitIM·Cell       │
│ ┌────────────────────────────┐ │
│ │   ╱╲    ╱╲╱                │ │ ← HoverCard 放大图 ~240×64
│ │  ╱  ╲__╱                   │ │
│ └────────────────────────────┘ │
│ 近 30 天 · 峰值 18             │
└────────────────────────────────┘
```

**Inline trigger** 规格：

- 小 sparkline，~40×16 SVG `<path>`，`stroke="currentColor"`，色值 `text-primary`
- 紧邻数字 `<span class="text-xs font-mono">{today}</span>`
- 鼠标 hover 延时 200ms 展开 HoverCard（Radix 默认行为）
- 按钮样式对齐现有顶栏 icon button 的圆角/高亮风格（`h-7 rounded-md hover:bg-surface/60`），宽度自适应 sparkline + 数字内容

**HoverCard 内容**：

- 标题：`{todayDau} 人正在使用 GitIM·Cell`
- 大 sparkline，同一 `<SparklinePath />` 组件尺寸参数变大
- 脚注：`近 30 天 · 峰值 {max(dau)}`
- `align="end" sideOffset={4}`，和 `UpdateIndicator` 的 popover 对齐

**Sparkline 算法**（纯函数，单测覆盖）：

```ts
export function sparklinePath(values: number[], w: number, h: number): string {
  const max = Math.max(1, ...values);  // 全 0 时避免除 0，画成底部平线
  const step = w / (values.length - 1);
  return values
    .map((v, i) => {
      const x = i * step;
      const y = h - (v / max) * h;
      return `${i === 0 ? "M" : "L"}${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(" ");
}
```

### 挂载位置

`webui-v2/src/components/layout/app-shell.tsx` 右侧集群，`<UpdateIndicator />` 之前（update 告警优先级更高）：

```tsx
<div className="flex items-center justify-end gap-2 min-w-[140px]">
  <ThemeToggle />
  <a href="https://x.com/arknights60" ...>...</a>
  <UsageIndicator />   {/* ← 新增 */}
  <UpdateIndicator />
  <button onClick={() => navigate("/docs")} ...>...</button>
  {currentUser ? <span>...</span> : null}
</div>
```

---

## 5. 测试策略

### Backend（Vitest + miniflare，沿用 cell-api 现有约定）

| 场景 | 断言 |
|------|------|
| `visits` upsert 幂等 | 同 UUID 同一天 insert 两次，表里一行 |
| `recordVisitor` 并行写 | `visitors` 和 `visits` 都写入成功 |
| `/api/stats` 返回 30 天 | `days.length === 30`，没数据的天 dau=0 |
| `/api/stats` 公开 | 无 `x-admin-secret` 返回 200 |
| `/admin/stats` 仍鉴权 | 无 secret 返回 401 |
| `/admin/stats` 中文 shape | `每日 / 汇总` 结构完整，`30天粘性` ∈ [0, 1] |
| Cache header | `/api/stats` 有 `Cache-Control: public, max-age=60` |

### Frontend

| 场景 | 断言 |
|------|------|
| `sparklinePath` 全 0 | 生成底部平线（所有 y ≈ h） |
| `sparklinePath` 单调递增 | path 点从左下到右上 |
| `sparklinePath` 峰值 | 最高点 y ≈ 0 |
| `UsageIndicator` `days=null` | 不渲染任何 DOM |
| `UsageIndicator` `days` 有值 | 渲染 sparkline + 今日数字 |

Hover 行为、Radix 弹层不测（信任 Radix）。

---

## 6. 上线顺序

1. **Backend 先**：migration `0002` + `visits` 写入 + `/api/stats` + `/admin/stats` 改造
   - `wrangler deploy` 到 Cloudflare
   - migration 通过 wrangler CLI 或控制台执行
2. **等 24h+**：让 `visits` 表攒到第一天真实数据
   - migration 跑完时 `visits` 是空表，24h 后图表才不是全 0
   - 可接受：也可以直接上前端展示"确实是空的"诚实状态，按实际情况选择
3. **Frontend 后**：`UsageIndicator` + `hover-card.tsx` + lib/hook 扩展
   - `npm run deploy`（已有 `wrangler pages deploy` script）

### 回滚

- Backend：`/api/stats` 改 400/下线即可；`visits` 表保留不清理（下次上线可直接复用）
- Frontend：fetchStats 失败已做 null-safe 隐身，回滚只要不 render 组件或 revert commit

---

## 7. 开放点 / 未决事项

以下全部列为 **v2+ 再说**：

- `visits` 保留期 / 自动清理 → v1 不做，到 3 个月后再看体量
- `/api/stats` 按 UA 过滤爬虫 / 机器人 → 当下访客源 = 真实 runtime + 极少数 cell.gitim.io 访客，机器人噪音低
- 粘性指标暴露到公开端点 → v1 公开端点只给 `dau`，粘性留 admin
- 多语言 UI → 不做
