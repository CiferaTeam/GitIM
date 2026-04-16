import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/verify", async (c) => {
  const body = await c.req.json<{ code?: string; device_id?: string }>();
  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "missing code or device_id" }, 400);
  }
  if (code.length > 64) {
    return c.json({ ok: false, error: "code too long" }, 400);
  }

  const raw = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "invalid code" }, 403);
  }

  const invite: InviteCode = JSON.parse(raw);

  // Already registered device — update last_seen
  const existing = invite.devices.find((d) => d.id === deviceId);
  if (existing) {
    existing.last_seen = new Date().toISOString();
    await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
    return c.json({ ok: true });
  }

  // New device — check limit
  if (invite.devices.length >= invite.max_devices) {
    return c.json({ ok: false, error: "device limit reached" }, 403);
  }

  const now = new Date().toISOString();
  invite.devices.push({ id: deviceId, registered_at: now, last_seen: now });
  await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

export { app as inviteRoutes };
