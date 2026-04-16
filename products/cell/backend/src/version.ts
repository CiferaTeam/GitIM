import { Hono } from "hono";
import type { Bindings } from "./types";

const GITHUB_RELEASES_URL =
  "https://api.github.com/repos/CiferaTeam/gitim-releases/releases/latest";
const VERSION_CACHE_KEY = "cache:latest_version";
const VERSION_CACHE_TTL = 3600; // 1 hour

async function fetchLatestVersion(kv: KVNamespace): Promise<string | null> {
  // Check KV cache first
  const cached = await kv.get(VERSION_CACHE_KEY);
  if (cached) return cached;

  // Fetch from GitHub
  try {
    const res = await fetch(GITHUB_RELEASES_URL, {
      headers: {
        "User-Agent": "cell-api",
        Accept: "application/vnd.github+json",
      },
    });
    if (!res.ok) return null;

    const data = (await res.json()) as { tag_name?: string };
    const tag = data.tag_name;
    if (!tag) return null;

    // Strip leading "v" → "0.4.2"
    const version = tag.startsWith("v") ? tag.slice(1) : tag;

    // Cache in KV with TTL
    await kv.put(VERSION_CACHE_KEY, version, { expirationTtl: VERSION_CACHE_TTL });
    return version;
  } catch {
    return null;
  }
}

async function recordVisitor(
  uuid: string,
  kv: KVNamespace,
  db: D1Database,
): Promise<void> {
  const kvKey = `visitor:${uuid}`;
  const now = new Date().toISOString();

  // KV: simple last_seen update
  await kv.put(kvKey, now);

  // D1: upsert visitor
  await db
    .prepare(
      `INSERT INTO visitors (uuid, first_seen, last_seen, visit_count)
       VALUES (?1, ?2, ?2, 1)
       ON CONFLICT(uuid) DO UPDATE SET
         last_seen = ?2,
         visit_count = visit_count + 1`,
    )
    .bind(uuid, now)
    .run();
}

const app = new Hono<{ Bindings: Bindings }>();

app.post("/api/check-version", async (c) => {
  let body: { uuid?: string };
  try {
    body = await c.req.json();
  } catch {
    return c.json({ ok: false, error: "invalid request" }, 400);
  }

  const uuid = body.uuid?.trim();
  if (!uuid) {
    return c.json({ ok: false, error: "uuid required" }, 400);
  }

  // Record visitor (fire-and-forget, don't block response)
  c.executionCtx.waitUntil(
    recordVisitor(uuid, c.env.CELL_GITIM_KV, c.env.CELL_DB),
  );

  const latestVersion = await fetchLatestVersion(c.env.CELL_GITIM_KV);

  return c.json({
    ok: true,
    latest_version: latestVersion,
  });
});

export { app as versionRoutes };
