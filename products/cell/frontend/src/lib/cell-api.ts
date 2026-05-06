const API_URL = import.meta.env.VITE_CELL_API_URL ?? "";

interface VerifyResult {
  ok: boolean;
  error?: string;
}

export async function verifyInviteCode(
  code: string,
  deviceId: string
): Promise<VerifyResult> {
  let res: Response;
  try {
    res = await fetch(`${API_URL}/api/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code, device_id: deviceId }),
    });
  } catch {
    return { ok: false, error: "无法连接验证服务" };
  }

  try {
    return (await res.json()) as VerifyResult;
  } catch {
    return { ok: false, error: `服务器返回异常 (${res.status})` };
  }
}

interface VersionResult {
  ok: boolean;
  latest_version?: string;
  error?: string;
}

export async function checkVersion(uuid: string): Promise<VersionResult> {
  try {
    const res = await fetch(`${API_URL}/api/check-version`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ uuid }),
    });
    return (await res.json()) as VersionResult;
  } catch {
    return { ok: false, error: "unable to reach version service" };
  }
}

export interface StatsDay {
  date: string;
  dau: number;
}

export async function fetchStats(): Promise<StatsDay[] | null> {
  try {
    const res = await fetch(`${API_URL}/api/stats`);
    if (!res.ok) return null;
    const body = (await res.json()) as { days?: StatsDay[] };
    return body.days ?? null;
  } catch {
    return null;
  }
}
