import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

// Auth middleware — scoped to /admin/* so it doesn't leak to sibling routes
// when mounted via app.route("/", adminRoutes).
app.use("/admin/*", async (c, next) => {
  if (c.req.header("x-admin-secret") !== c.env.ADMIN_SECRET) {
    return c.json({ error: "unauthorized" }, 401);
  }
  await next();
});

// List all invite codes
app.get("/admin/codes", async (c) => {
  const list = await c.env.CELL_GITIM_KV.list({ prefix: "invite:" });
  const codes: InviteCode[] = [];
  for (const key of list.keys) {
    const raw = await c.env.CELL_GITIM_KV.get(key.name);
    if (raw) codes.push(JSON.parse(raw));
  }
  return c.json({ codes });
});

// Create invite code
app.post("/admin/codes", async (c) => {
  const body = await c.req.json<{
    code?: string;
    note?: string;
    max_devices?: number;
  }>();
  const code = body.code?.trim();

  if (!code || code.length > 64) {
    return c.json({ error: "code required, max 64 chars" }, 400);
  }

  const existing = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (existing) {
    return c.json({ error: "code already exists" }, 409);
  }

  const invite: InviteCode = {
    code,
    created_at: new Date().toISOString(),
    max_devices: body.max_devices ?? 5,
    note: body.note ?? "",
    devices: [],
  };

  await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true, invite }, 201);
});

// Get code detail
app.get("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  const raw = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);
  return c.json(JSON.parse(raw));
});

// Delete code
app.delete("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  await c.env.CELL_GITIM_KV.delete(kvKey(code));
  return c.json({ ok: true });
});

// Remove a device from a code (manual reset)
app.delete("/admin/codes/:code/devices/:deviceId", async (c) => {
  const code = c.req.param("code");
  const deviceId = c.req.param("deviceId");

  const raw = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);

  const invite: InviteCode = JSON.parse(raw);
  invite.devices = invite.devices.filter((d) => d.id !== deviceId);
  await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

// Last-30-day new-UV series + cumulative growth curve
// Baseline: 30 days ago = 0, cumulative adds each day's new UV up to today.
app.get("/admin/stats", async (c) => {
  const today = new Date();
  today.setUTCHours(0, 0, 0, 0);
  const startMs = today.getTime() - 29 * 86400_000;
  const startIso = new Date(startMs).toISOString();

  const rows = await c.env.CELL_DB
    .prepare(
      `SELECT DATE(first_seen) AS day, COUNT(*) AS new_uv
       FROM visitors
       WHERE first_seen >= ?1
       GROUP BY DATE(first_seen)`,
    )
    .bind(startIso)
    .all<{ day: string; new_uv: number }>();

  const byDay = new Map<string, number>();
  for (const r of rows.results ?? []) byDay.set(r.day, r.new_uv);

  const days: { date: string; new_uv: number; cumulative_new_uv: number }[] = [];
  let cumulative = 0;
  for (let i = 0; i < 30; i++) {
    const date = new Date(startMs + i * 86400_000).toISOString().slice(0, 10);
    const new_uv = byDay.get(date) ?? 0;
    cumulative += new_uv;
    days.push({ date, new_uv, cumulative_new_uv: cumulative });
  }

  return c.json({ days });
});

export { app as adminRoutes };
