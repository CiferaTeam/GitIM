import { Hono } from "hono";
import type { Bindings } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

// Public 30-day DAU — no auth. Cached 60s at the edge so bursts of frontend
// loads don't walk the D1 aggregation repeatedly.
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
