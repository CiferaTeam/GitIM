import type Database from "better-sqlite3";
import {
  dmThreadName,
  ensureChannel,
  ensureDmThread,
  nextLineNumber,
  validateHandler,
} from "./db.js";

// ── GitIM API Types ─────────────────────────────────────────
//
//  Request:  { method: string, ...params }
//  Response: { ok: boolean, data?: any, error?: string }
//
//  和 gitim-daemon/src/api.rs 的 Request enum 完全对齐：
//    send, read, channels, users, thread, status, subscribe, stop, register_user

export interface Response {
  ok: boolean;
  data?: unknown;
  error?: string;
}

function success(data: unknown): Response {
  return { ok: true, data };
}

function error(msg: string): Response {
  return { ok: false, error: msg };
}

// ── Request Router ──────────────────────────────────────────
//
//  POST /api  →  JSON { method, ...params }  →  Response
//
//  ┌─────────────┐
//  │   Request    │
//  │  { method }  │──┬── send ──────────► handleSend
//  └─────────────┘  ├── read ──────────► handleRead
//                   ├── channels ──────► handleListChannels
//                   ├── users ─────────► handleListUsers
//                   ├── thread ────────► handleGetThread
//                   ├── register_user ─► handleRegisterUser
//                   ├── status ────────► handleStatus
//                   ├── subscribe ─────► (stub)
//                   └── stop ──────────► handleStop

export function handleRequest(
  db: Database.Database,
  req: Record<string, unknown>
): Response {
  const method = req.method as string;
  switch (method) {
    case "send":
      return handleSend(db, req);
    case "read":
      return handleRead(db, req);
    case "channels":
      return handleListChannels(db);
    case "users":
      return handleListUsers(db);
    case "thread":
      return handleGetThread(db, req);
    case "register_user":
      return handleRegisterUser(db, req);
    case "status":
      return handleStatus();
    case "subscribe":
      return success({ subscribed: true });
    case "stop":
      return handleStop();
    default:
      return error(`unknown method: ${method}`);
  }
}

// ── Handlers ────────────────────────────────────────────────

function resolveThread(
  db: Database.Database,
  channel: string
): { name: string; type: "channel" | "dm" } | Response {
  if (channel.startsWith("dm:")) {
    const parts = channel.slice(3).split(",");
    if (parts.length !== 2) return error("DM format must be dm:handler1,handler2");
    const [h1, h2] = parts;
    const err1 = validateHandler(h1);
    if (err1) return error(`invalid DM handler: ${err1}`);
    const err2 = validateHandler(h2);
    if (err2) return error(`invalid DM handler: ${err2}`);
    const name = dmThreadName(h1, h2);
    ensureDmThread(db, name);
    return { name, type: "dm" };
  }
  ensureChannel(db, channel);
  return { name: channel, type: "channel" };
}

function handleSend(db: Database.Database, req: Record<string, unknown>): Response {
  const channel = req.channel as string;
  const body = req.body as string;
  const replyTo = (req.reply_to as number) ?? 0;
  const author = req.author as string | undefined;

  if (!channel || !body) return error("channel and body are required");
  if (!author) return error("no author specified");

  // Validate author
  const handlerErr = validateHandler(author);
  if (handlerErr) return error(`invalid author: ${handlerErr}`);

  // Check author is registered
  const user = db
    .prepare("SELECT handler FROM users WHERE handler = ?")
    .get(author);
  if (!user) return error(`unknown user: ${author}`);

  // Resolve thread
  const resolved = resolveThread(db, channel);
  if ("ok" in resolved) return resolved;

  // Serialized write: get next line number + insert in one transaction
  const insertMsg = db.transaction(() => {
    const lineNumber = nextLineNumber(db, resolved.name);
    db.prepare(
      `INSERT INTO messages (thread_name, thread_type, line_number, parent_line, handler, body)
       VALUES (?, ?, ?, ?, ?, ?)`
    ).run(resolved.name, resolved.type, lineNumber, replyTo, author, body);
    return lineNumber;
  });

  const lineNumber = insertMsg();

  return success({
    line_number: lineNumber,
    channel: resolved.name,
    status: "committed",
  });
}

function handleRead(db: Database.Database, req: Record<string, unknown>): Response {
  const channel = req.channel as string;
  const limit = req.limit as number | undefined;
  const since = req.since as number | undefined;

  if (!channel) return error("channel is required");

  const resolved = resolveThread(db, channel);
  if ("ok" in resolved) return resolved;

  let query = "SELECT * FROM messages WHERE thread_name = ?";
  const params: unknown[] = [resolved.name];

  if (since != null) {
    query += " AND line_number > ?";
    params.push(since);
  }

  query += " ORDER BY line_number ASC";

  if (limit != null) {
    // GitIM: returns last N messages (tail)
    const countRow = db
      .prepare(
        `SELECT COUNT(*) as cnt FROM messages WHERE thread_name = ?${since != null ? " AND line_number > ?" : ""}`
      )
      .get(...(since != null ? [resolved.name, since] : [resolved.name])) as { cnt: number };
    const offset = Math.max(0, countRow.cnt - limit);
    query += ` LIMIT ? OFFSET ?`;
    params.push(limit, offset);
  }

  const rows = db.prepare(query).all(...params) as Array<{
    line_number: number;
    parent_line: number;
    handler: string;
    created_at: string;
    body: string;
  }>;

  const messages = rows.map((r) => ({
    line_number: r.line_number,
    point_to: r.parent_line,
    author: r.handler,
    timestamp: r.created_at,
    body: r.body,
  }));

  return success({ channel, messages });
}

function handleListChannels(db: Database.Database): Response {
  const rows = db
    .prepare("SELECT name FROM channels ORDER BY name")
    .all() as Array<{ name: string }>;
  return success({ channels: rows.map((r) => r.name) });
}

function handleListUsers(db: Database.Database): Response {
  const rows = db
    .prepare("SELECT handler FROM users ORDER BY handler")
    .all() as Array<{ handler: string }>;
  return success({ users: rows.map((r) => r.handler) });
}

function handleGetThread(
  db: Database.Database,
  req: Record<string, unknown>
): Response {
  const channel = req.channel as string;
  const lineNumber = req.line_number as number;

  if (!channel || lineNumber == null) return error("channel and line_number are required");

  // Recursive: find root message + all descendants via parent_line chain
  const allMsgs = db
    .prepare("SELECT * FROM messages WHERE thread_name = ? ORDER BY line_number")
    .all(channel) as Array<{
    line_number: number;
    parent_line: number;
    handler: string;
    created_at: string;
    body: string;
  }>;

  const threadMsgs: typeof allMsgs = [];
  const visited = new Set<number>();
  const stack = [lineNumber];

  while (stack.length > 0) {
    const target = stack.pop()!;
    if (visited.has(target)) continue;
    visited.add(target);
    for (const msg of allMsgs) {
      if (msg.line_number === target || msg.parent_line === target) {
        threadMsgs.push(msg);
        if (msg.line_number !== target) stack.push(msg.line_number);
      }
    }
  }

  // Deduplicate and sort
  const seen = new Set<number>();
  const unique = threadMsgs.filter((m) => {
    if (seen.has(m.line_number)) return false;
    seen.add(m.line_number);
    return true;
  });
  unique.sort((a, b) => a.line_number - b.line_number);

  return success({
    channel,
    root_line: lineNumber,
    messages: unique.map((m) => ({
      line_number: m.line_number,
      point_to: m.parent_line,
      author: m.handler,
      timestamp: m.created_at,
      body: m.body,
    })),
  });
}

function handleRegisterUser(
  db: Database.Database,
  req: Record<string, unknown>
): Response {
  const handler = req.handler as string;
  const displayName = req.display_name as string;
  const role = (req.role as string) ?? "member";
  const introduction = (req.introduction as string) ?? "GitIM user";

  if (!handler || !displayName) return error("handler and display_name are required");

  const handlerErr = validateHandler(handler);
  if (handlerErr) return error(`invalid handler: ${handlerErr}`);

  // Check if already exists
  const existing = db
    .prepare("SELECT handler FROM users WHERE handler = ?")
    .get(handler);
  if (existing) {
    return success({ handler, exists: true });
  }

  db.prepare(
    "INSERT INTO users (handler, display_name, role, introduction) VALUES (?, ?, ?, ?)"
  ).run(handler, displayName, role, introduction);

  return success({ handler, exists: false });
}

function handleStatus(): Response {
  return success({ version: "0.1.0-mock", status: "running" });
}

function handleStop(): Response {
  // Schedule exit after response is sent
  setTimeout(() => process.exit(0), 100);
  return success({ status: "stopping" });
}
