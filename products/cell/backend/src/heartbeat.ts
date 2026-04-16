import { Hono } from "hono";
import type { Bindings, InviteCode } from "./types";
import { kvKey } from "./types";

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/heartbeat", async (c) => {
  let body: { code?: string; device_id?: string; version?: string };
  try {
    body = await c.req.json();
  } catch {
    return c.json({ ok: false, error: "请求格式错误" }, 400);
  }

  const code = body.code?.trim();
  const deviceId = body.device_id?.trim();

  if (!code || !deviceId) {
    return c.json({ ok: false, error: "缺少必要参数" }, 400);
  }

  const raw = await c.env.CELL_GITIM_KV.get(kvKey(code));
  if (!raw) {
    return c.json({ ok: false, error: "口诀无效" }, 403);
  }

  let invite: InviteCode;
  try {
    invite = JSON.parse(raw);
  } catch {
    return c.json({ ok: false, error: "服务端数据异常" }, 500);
  }

  const device = invite.devices.find((d) => d.id === deviceId);
  if (!device) {
    return c.json({ ok: false, error: "设备未注册" }, 403);
  }

  device.last_seen = new Date().toISOString();
  await c.env.CELL_GITIM_KV.put(kvKey(code), JSON.stringify(invite));

  return c.json({ ok: true, latest_version: body.version });
});

export { app as heartbeatRoutes };
