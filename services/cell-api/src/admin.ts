import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

// Auth middleware — all /admin/* routes require X-Admin-Secret header
app.use("*", async (c, next) => {
  if (c.req.header("x-admin-secret") !== c.env.ADMIN_SECRET) {
    return c.json({ error: "unauthorized" }, 401);
  }
  await next();
});

// List all invite codes
app.get("/admin/codes", async (c) => {
  const list = await c.env.CELL_KV.list({ prefix: "invite:" });
  const codes: InviteCode[] = [];
  for (const key of list.keys) {
    const raw = await c.env.CELL_KV.get(key.name);
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

  const existing = await c.env.CELL_KV.get(kvKey(code));
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

  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true, invite }, 201);
});

// Get code detail
app.get("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);
  return c.json(JSON.parse(raw));
});

// Delete code
app.delete("/admin/codes/:code", async (c) => {
  const code = c.req.param("code");
  await c.env.CELL_KV.delete(kvKey(code));
  return c.json({ ok: true });
});

// Remove a device from a code (manual reset)
app.delete("/admin/codes/:code/devices/:deviceId", async (c) => {
  const code = c.req.param("code");
  const deviceId = c.req.param("deviceId");

  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) return c.json({ error: "not found" }, 404);

  const invite: InviteCode = JSON.parse(raw);
  invite.devices = invite.devices.filter((d) => d.id !== deviceId);
  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

export { app as adminRoutes };
