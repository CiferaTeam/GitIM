import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/heartbeat", async (c) => {
  const body = await c.req.json<{
    code?: string;
    device_id?: string;
    version?: string;
  }>();
  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "missing fields" }, 400);
  }

  const raw = await c.env.CELL_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "invalid code" }, 403);
  }

  const invite: InviteCode = JSON.parse(raw);
  const device = invite.devices.find((d) => d.id === deviceId);
  if (!device) {
    return c.json({ ok: false, error: "device not registered" }, 403);
  }

  device.last_seen = new Date().toISOString();
  await c.env.CELL_KV.put(kvKey(code), JSON.stringify(invite));

  return c.json({ ok: true, latest_version: body.version });
});

export { app as heartbeatRoutes };
