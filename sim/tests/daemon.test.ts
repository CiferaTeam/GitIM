import { describe, it, before, after } from "node:test";
import assert from "node:assert/strict";
import { openDb, validateHandler, dmThreadName } from "../src/db.js";
import { handleRequest } from "../src/handlers.js";
import type Database from "better-sqlite3";

let db: Database.Database;

before(() => {
  // In-memory SQLite for tests
  const store = openDb(":memory:");
  db = store.db;
});

after(() => {
  db.close();
});

// ── Helper ──────────────────────────────────────────────────

function req(method: string, params: Record<string, unknown> = {}) {
  return handleRequest(db, { method, ...params });
}

// ── validateHandler ─────────────────────────────────────────

describe("validateHandler", () => {
  it("accepts valid handlers", () => {
    assert.equal(validateHandler("alice"), null);
    assert.equal(validateHandler("bob-123"), null);
    assert.equal(validateHandler("a"), null);
  });

  it("rejects empty handler", () => {
    assert.notEqual(validateHandler(""), null);
  });

  it("rejects uppercase", () => {
    assert.notEqual(validateHandler("Alice"), null);
  });

  it("rejects reserved 'system'", () => {
    assert.notEqual(validateHandler("system"), null);
  });

  it("rejects too long", () => {
    assert.notEqual(validateHandler("a".repeat(40)), null);
  });
});

// ── dmThreadName ────────────────────────────────────────────

describe("dmThreadName", () => {
  it("sorts alphabetically", () => {
    assert.equal(dmThreadName("bob", "alice"), "alice--bob");
    assert.equal(dmThreadName("alice", "bob"), "alice--bob");
  });
});

// ── status ──────────────────────────────────────────────────

describe("status", () => {
  it("returns running", () => {
    const res = req("status");
    assert.equal(res.ok, true);
    assert.equal((res.data as any).status, "running");
  });
});

// ── register_user ───────────────────────────────────────────

describe("register_user", () => {
  it("registers a new user", () => {
    const res = req("register_user", {
      handler: "alice",
      display_name: "Alice",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).handler, "alice");
    assert.equal((res.data as any).exists, false);
  });

  it("returns exists=true for duplicate", () => {
    const res = req("register_user", {
      handler: "alice",
      display_name: "Alice",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).exists, true);
  });

  it("rejects invalid handler", () => {
    const res = req("register_user", {
      handler: "INVALID",
      display_name: "Bad",
    });
    assert.equal(res.ok, false);
    assert.match(res.error!, /invalid handler/);
  });

  it("rejects reserved 'system'", () => {
    const res = req("register_user", {
      handler: "system",
      display_name: "System",
    });
    assert.equal(res.ok, false);
  });
});

// ── send ────────────────────────────────────────────────────

describe("send", () => {
  before(() => {
    // Ensure bob is registered for send tests
    req("register_user", { handler: "bob", display_name: "Bob" });
  });

  it("sends a message to a channel", () => {
    const res = req("send", {
      channel: "general",
      body: "hello world",
      author: "alice",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).line_number, 1);
    assert.equal((res.data as any).channel, "general");
  });

  it("auto-increments line numbers", () => {
    const res = req("send", {
      channel: "general",
      body: "second message",
      author: "bob",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).line_number, 2);
  });

  it("auto-creates channel", () => {
    const res = req("send", {
      channel: "new-channel",
      body: "first in new channel",
      author: "alice",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).line_number, 1);

    // Verify channel was created
    const channels = req("channels");
    assert.ok((channels.data as any).channels.includes("new-channel"));
  });

  it("rejects unregistered author", () => {
    const res = req("send", {
      channel: "general",
      body: "ghost message",
      author: "unknown-user",
    });
    assert.equal(res.ok, false);
    assert.match(res.error!, /unknown user/);
  });

  it("rejects invalid handler format", () => {
    const res = req("send", {
      channel: "general",
      body: "bad author",
      author: "UPPERCASE",
    });
    assert.equal(res.ok, false);
    assert.match(res.error!, /invalid author/);
  });

  it("supports reply_to", () => {
    const res = req("send", {
      channel: "general",
      body: "replying to first message",
      author: "alice",
      reply_to: 1,
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).line_number, 3);
  });
});

// ── read ────────────────────────────────────────────────────

describe("read", () => {
  it("reads all messages from a channel", () => {
    const res = req("read", { channel: "general" });
    assert.equal(res.ok, true);
    const msgs = (res.data as any).messages;
    assert.ok(msgs.length >= 3);
    assert.equal(msgs[0].author, "alice");
    assert.equal(msgs[0].body, "hello world");
  });

  it("filters by since", () => {
    const res = req("read", { channel: "general", since: 1 });
    assert.equal(res.ok, true);
    const msgs = (res.data as any).messages;
    assert.ok(msgs.every((m: any) => m.line_number > 1));
  });

  it("limits results (tail)", () => {
    const res = req("read", { channel: "general", limit: 1 });
    assert.equal(res.ok, true);
    const msgs = (res.data as any).messages;
    assert.equal(msgs.length, 1);
    // Should be the last message
    assert.equal(msgs[0].line_number, 3);
  });

  it("returns empty for non-existent channel", () => {
    const res = req("read", { channel: "nonexistent" });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).messages.length, 0);
  });
});

// ── channels ────────────────────────────────────────────────

describe("channels", () => {
  it("lists created channels", () => {
    const res = req("channels");
    assert.equal(res.ok, true);
    const channels = (res.data as any).channels as string[];
    assert.ok(channels.includes("general"));
    assert.ok(channels.includes("new-channel"));
  });
});

// ── users ───────────────────────────────────────────────────

describe("users", () => {
  it("lists registered users", () => {
    const res = req("users");
    assert.equal(res.ok, true);
    const users = (res.data as any).users as string[];
    assert.ok(users.includes("alice"));
    assert.ok(users.includes("bob"));
  });

  it("returns sorted list", () => {
    const res = req("users");
    const users = (res.data as any).users as string[];
    const sorted = [...users].sort();
    assert.deepEqual(users, sorted);
  });
});

// ── thread ──────────────────────────────────────────────────

describe("thread", () => {
  it("gets thread with replies", () => {
    const res = req("thread", { channel: "general", line_number: 1 });
    assert.equal(res.ok, true);
    const msgs = (res.data as any).messages;
    // Should include line 1 (root) and line 3 (reply to 1)
    const lineNumbers = msgs.map((m: any) => m.line_number);
    assert.ok(lineNumbers.includes(1));
    assert.ok(lineNumbers.includes(3));
  });
});

// ── DM ──────────────────────────────────────────────────────

describe("dm", () => {
  it("sends DM via dm: prefix", () => {
    const res = req("send", {
      channel: "dm:alice,bob",
      body: "hey bob, private message",
      author: "alice",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).channel, "alice--bob");
  });

  it("normalizes DM order", () => {
    const res = req("send", {
      channel: "dm:bob,alice",
      body: "hey alice back",
      author: "bob",
    });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).channel, "alice--bob");
  });

  it("reads DM messages", () => {
    const res = req("read", { channel: "dm:alice,bob" });
    assert.equal(res.ok, true);
    assert.equal((res.data as any).messages.length, 2);
  });

  it("DMs don't appear in channel list", () => {
    const res = req("channels");
    const channels = (res.data as any).channels as string[];
    assert.ok(!channels.includes("alice--bob"));
  });
});

// ── unknown method ──────────────────────────────────────────

describe("unknown method", () => {
  it("returns error for unknown method", () => {
    const res = req("nonexistent");
    assert.equal(res.ok, false);
    assert.match(res.error!, /unknown method/);
  });
});
