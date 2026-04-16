import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/verify", async (c) => {
  let body: { code?: string; device_id?: string };
  try {
    body = await c.req.json();
  } catch {
    return c.json({ ok: false, error: "请求格式错误" }, 400);
  }

  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "缺少口诀或设备信息" }, 400);
  }
  if (code.length > 64) {
    return c.json({ ok: false, error: "口诀长度不能超过 64 字符" }, 400);
  }

  const raw = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "口诀无效" }, 403);
  }

  let invite: InviteCode;
  try {
    invite = JSON.parse(raw);
  } catch {
    return c.json({ ok: false, error: "服务端数据异常，请联系管理员" }, 500);
  }

  // Already registered device — update last_seen
  const existing = invite.devices.find((d) => d.id === deviceId);
  if (existing) {
    existing.last_seen = new Date().toISOString();
    await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
    return c.json({ ok: true });
  }

  // New device — check limit
  if (invite.devices.length >= invite.max_devices) {
    return c.json(
      { ok: false, error: `设备配额已满（上限 ${invite.max_devices} 台）` },
      403
    );
  }

  const now = new Date().toISOString();
  invite.devices.push({ id: deviceId, registered_at: now, last_seen: now });
  await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));
  return c.json({ ok: true });
});

export { app as inviteRoutes };
