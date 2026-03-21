import Database from "better-sqlite3";
import path from "node:path";

// ── Schema ──────────────────────────────────────────────────
//
//   channels (name PK)
//   dm_threads (name PK)           -- alice--bob 格式
//   users (handler PK)
//   messages (id, thread_name, line_number, parent_line, handler, body, ts)
//
//   GitIM 语义：
//     channel 消息  → thread_name = channel name
//     DM 消息       → thread_name = dm_threads.name
//     line_number   → 每个 thread 内自增
//     parent_line   → 回复引用（0 = 顶层消息）

const SCHEMA = `
CREATE TABLE IF NOT EXISTS channels (
  name TEXT PRIMARY KEY,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS dm_threads (
  name TEXT PRIMARY KEY,
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS users (
  handler TEXT PRIMARY KEY CHECK(length(handler) BETWEEN 1 AND 39),
  display_name TEXT NOT NULL,
  role TEXT NOT NULL DEFAULT 'member',
  introduction TEXT NOT NULL DEFAULT 'GitIM user',
  created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE IF NOT EXISTS messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  thread_name TEXT NOT NULL,
  thread_type TEXT NOT NULL CHECK(thread_type IN ('channel', 'dm')),
  line_number INTEGER NOT NULL,
  parent_line INTEGER NOT NULL DEFAULT 0,
  handler TEXT NOT NULL REFERENCES users(handler),
  body TEXT NOT NULL,
  created_at TEXT NOT NULL DEFAULT (datetime('now')),
  UNIQUE(thread_name, line_number)
);

CREATE INDEX IF NOT EXISTS idx_messages_thread ON messages(thread_name, line_number);
`;

export interface Db {
  db: Database.Database;
  close(): void;
}

export function openDb(dbPath?: string): Db {
  const resolvedPath = dbPath ?? path.join(process.cwd(), "gitim-sim.db");
  const db = new Database(resolvedPath);
  db.pragma("journal_mode = WAL");
  db.pragma("foreign_keys = ON");
  db.exec(SCHEMA);
  return { db, close: () => db.close() };
}

// ── Helpers ─────────────────────────────────────────────────

/** DM thread name: sort handlers alphabetically, join with -- */
export function dmThreadName(h1: string, h2: string): string {
  return [h1, h2].sort().join("--");
}

/** Validate handler format: a-z 0-9 hyphen, 1-39 chars, not "system" */
export function validateHandler(handler: string): string | null {
  if (!/^[a-z0-9-]{1,39}$/.test(handler)) {
    return "handler must be 1-39 chars of a-z, 0-9, or hyphen";
  }
  if (handler === "system") {
    return "'system' is a reserved handler";
  }
  return null;
}

/** Next line number for a thread (serialized — called within transaction) */
export function nextLineNumber(db: Database.Database, threadName: string): number {
  const row = db.prepare(
    "SELECT MAX(line_number) as max_line FROM messages WHERE thread_name = ?"
  ).get(threadName) as { max_line: number | null } | undefined;
  return (row?.max_line ?? 0) + 1;
}

/** Ensure channel exists, auto-create if not */
export function ensureChannel(db: Database.Database, name: string): void {
  db.prepare("INSERT OR IGNORE INTO channels (name) VALUES (?)").run(name);
}

/** Ensure DM thread exists, auto-create if not */
export function ensureDmThread(db: Database.Database, name: string): void {
  db.prepare("INSERT OR IGNORE INTO dm_threads (name) VALUES (?)").run(name);
}
