import { Hono } from "hono";
import type { Bindings } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

// Auth middleware — scoped to /admin/* so it doesn't leak to sibling routes
// when mounted via app.route("/", adminRoutes).
app.use("/admin/*", async (c, next) => {
  if (c.req.header("x-admin-secret") !== c.env.ADMIN_SECRET) {
    return c.json({ error: "unauthorized" }, 401);
  }
  await next();
});

// Rich 30-day stats with per-day (new / returning / DAU / cumulative) and
// top-level summary (WAU / MAU / stickiness / total). Response uses Chinese
// keys — this endpoint is read by the operator directly, not by programs.
//
// Schema note: depends on the `visits(uuid, day)` log table created in
// migration 0002 — without it, DAU history is not recoverable from `visitors`
// alone (last_seen overwrites itself).
app.get("/admin/stats", async (c) => {
  const today = new Date();
  today.setUTCHours(0, 0, 0, 0);
  const startMs = today.getTime() - 29 * 86400_000;
  const startDay = new Date(startMs).toISOString().slice(0, 10);
  const startIso = new Date(startMs).toISOString();
  const todayDay = today.toISOString().slice(0, 10);
  const day7Start = new Date(today.getTime() - 6 * 86400_000)
    .toISOString()
    .slice(0, 10);

  // Per-day DAU from visits (window-bounded)
  const dauRows = await c.env.CELL_DB
    .prepare(
      `SELECT day, COUNT(*) AS dau
       FROM visits
       WHERE day >= ?1
       GROUP BY day`,
    )
    .bind(startDay)
    .all<{ day: string; dau: number }>();

  // Per-day new UV from visitors.first_seen (same 30-day window)
  const newUvRows = await c.env.CELL_DB
    .prepare(
      `SELECT DATE(first_seen) AS day, COUNT(*) AS new_uv
       FROM visitors
       WHERE first_seen >= ?1
       GROUP BY DATE(first_seen)`,
    )
    .bind(startIso)
    .all<{ day: string; new_uv: number }>();

  // 7-day WAU / 30-day MAU / all-time total
  const wauRes = await c.env.CELL_DB
    .prepare(`SELECT COUNT(DISTINCT uuid) AS n FROM visits WHERE day >= ?1`)
    .bind(day7Start)
    .first<{ n: number }>();

  const mauRes = await c.env.CELL_DB
    .prepare(`SELECT COUNT(DISTINCT uuid) AS n FROM visits WHERE day >= ?1`)
    .bind(startDay)
    .first<{ n: number }>();

  const totalRes = await c.env.CELL_DB
    .prepare(`SELECT COUNT(*) AS n FROM visitors`)
    .first<{ n: number }>();

  const dauByDay = new Map<string, number>();
  for (const r of dauRows.results ?? []) dauByDay.set(r.day, r.dau);
  const newByDay = new Map<string, number>();
  for (const r of newUvRows.results ?? []) newByDay.set(r.day, r.new_uv);

  type DailyRow = {
    日期: string;
    新增: number;
    回访: number;
    日活: number;
    累计新增: number;
  };

  const daily: DailyRow[] = [];
  let cumulativeNew = 0;
  let dauSum = 0;
  for (let i = 0; i < 30; i++) {
    const date = new Date(startMs + i * 86400_000).toISOString().slice(0, 10);
    const 新增 = newByDay.get(date) ?? 0;
    const 日活 = dauByDay.get(date) ?? 0;
    // Clamp to 0 — if backfilled visitors happen to out-count visits rows for
    // the day (e.g. partial migration state), don't render a negative回访.
    const 回访 = Math.max(0, 日活 - 新增);
    cumulativeNew += 新增;
    dauSum += 日活;
    daily.push({ 日期: date, 新增, 回访, 日活, 累计新增: cumulativeNew });
  }

  const activeToday = dauByDay.get(todayDay) ?? 0;
  const wau = wauRes?.n ?? 0;
  const mau = mauRes?.n ?? 0;
  const totalUv = totalRes?.n ?? 0;
  // DAU_avg / MAU — standard stickiness, 0 when there's no MAU yet.
  const stickiness =
    mau > 0 ? Number((dauSum / 30 / mau).toFixed(2)) : 0;

  return c.json({
    每日: daily,
    汇总: {
      今日活跃: activeToday,
      近7天活跃: wau,
      近30天活跃: mau,
      累计用户: totalUv,
      "30天粘性": stickiness,
    },
  });
});

export { app as adminRoutes };
