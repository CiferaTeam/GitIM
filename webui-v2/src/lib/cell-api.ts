const API_URL = import.meta.env.VITE_CELL_API_URL ?? "";

interface VerifyResult {
  ok: boolean;
  error?: string;
}

export async function verifyInviteCode(
  code: string,
  deviceId: string
): Promise<VerifyResult> {
  try {
    const res = await fetch(`${API_URL}/api/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device_id: deviceId }),
    });
    return (await res.json()) as VerifyResult;
  } catch {
    return { ok: false, error: "无法连接验证服务" };
  }
}

export async function sendHeartbeat(
  code: string,
  deviceId: string,
  version?: string
): Promise<void> {
  try {
    await fetch(`${API_URL}/api/heartbeat`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device_id: deviceId, version }),
    });
  } catch {
    // heartbeat failure is non-critical, silently ignore
  }
}
