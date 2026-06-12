// Real-wasm coverage for the daemon-web .thread paths after the convergence
// onto gitim-wasm. These run the authoritative Rust logic (no mock) and pin
// the Rust semantics that differ from the old hand-written TS — the whole
// point of the migration. The wasm singleton is initialized by the global
// test-setup-wasm.ts (vite.config.ts setupFiles).

import { describe, it, expect } from "vitest";
import { parseThread, type ParsedEvent, type ParsedMessage } from "./parser";
import { formatMessage, formatEvent } from "./formatter";
import { validateHandler } from "./paths";
import { resolveConflicts, extractThreadAdditions } from "./conflict";
import { parseChannelMeta, parseUserMeta } from "gitim-wasm";

describe("parseThread — Rust semantics via wasm", () => {
  it("parses a basic message", () => {
    const f = parseThread(
      "[L000001][P000000][@alice][20260317T120000Z] hello world\n",
    );
    expect(f.entries).toHaveLength(1);
    const m = f.entries[0] as ParsedMessage;
    expect(m.type).toBe("message");
    expect(m.line_number).toBe(1);
    expect(m.author).toBe("alice");
    expect(m.body).toBe("hello world");
  });

  it("accepts compound (kebab/snake) event-type tokens", () => {
    // Old TS allowed any charset but so does this; the meaningful change is
    // that the Rust charset is [a-z][a-z0-9_-]* — compound tokens parse.
    const f = parseThread(
      "[L000001][P000000][@alice][20260317T120000Z][E:leave-workspace] {}\n",
    );
    const e = f.entries[0] as ParsedEvent;
    expect(e.type).toBe("event");
    expect(e.event_type).toBe("leave-workspace");
  });

  it("treats an out-of-charset event token as continuation text", () => {
    // [E:Weird] (uppercase) does not match the Rust event charset, so the
    // whole line falls through and is appended to the prior message body.
    // (Old TS accepted any non-`]` token and would have parsed it as an
    // event — this is a deliberate behavior change.)
    const f = parseThread(
      "[L000001][P000000][@alice][20260317T120000Z] first\n" +
        "[L000002][P000000][@alice][20260317T120000Z][E:Weird] {}\n",
    );
    expect(f.entries).toHaveLength(1);
    const m = f.entries[0] as ParsedMessage;
    expect(m.body).toContain("[E:Weird]");
  });

  it("reads back the reserved system handler", () => {
    const f = parseThread(
      "[L000001][P000000][@system][20260317T120000Z][E:cron_fire] {}\n",
    );
    const e = f.entries[0] as ParsedEvent;
    expect(e.author).toBe("system");
    expect(e.event_type).toBe("cron_fire");
  });

  it("rejects a prefix line with an empty body (literal space + non-empty)", () => {
    // Old TS used `\s(.*)` (body could be empty) and would have parsed this.
    // Rust requires a literal space followed by a non-empty body, so the
    // line isn't a message start — as the first line, parsing fails.
    expect(() =>
      parseThread("[L000001][P000000][@alice][20260317T120000Z] \n"),
    ).toThrow();
  });
});

describe("validateHandler — Rust Handler::new via wasm", () => {
  it("accepts a valid handler", () => {
    expect(validateHandler("alice")).toBeNull();
    expect(validateHandler("eng-team-1")).toBeNull();
  });

  it("rejects a leading hyphen (old TS regex allowed this prefix shape)", () => {
    expect(validateHandler("-foo")).not.toBeNull();
  });

  it("rejects consecutive hyphens, reserved name, and over-length", () => {
    expect(validateHandler("a--b")).not.toBeNull();
    expect(validateHandler("system")).not.toBeNull();
    expect(validateHandler("a".repeat(40))).not.toBeNull();
  });
});

describe("formatMessage/formatEvent — round-trip through wasm", () => {
  it("round-trips a message through format -> parse", () => {
    const line = formatMessage(3, 1, "lewis", "20260317T120100Z", "reply body");
    const f = parseThread(line);
    const m = f.entries[0] as ParsedMessage;
    expect(m.line_number).toBe(3);
    expect(m.point_to).toBe(1);
    expect(m.author).toBe("lewis");
    expect(m.body).toBe("reply body");
  });

  it("formats an event with point_to forced to zero", () => {
    const line = formatEvent(5, "alice", "20260317T120000Z", "join", {});
    expect(line).toBe(
      "[L000005][P000000][@alice][20260317T120000Z][E:join] {}\n",
    );
  });
});

describe("resolveConflicts — wasm resolveContentPure + buildRebaseCommitMsg", () => {
  const remote =
    "[L000001][P000000][@alice][20260317T120000Z] base\n" +
    "[L000002][P000001][@alice][20260317T120050Z] remote\n";
  const localAdditions =
    "[L000002][P000001][@lewis][20260317T120100Z] local one\n" +
    "[L000003][P000002][@lewis][20260317T120200Z] local two\n";

  it("renumbers local additions after the remote's last line", () => {
    const resolved = resolveConflicts(
      { "channels/general.thread": localAdditions },
      { "channels/general.thread": remote },
    );
    expect(resolved.files["channels/general.thread"]).toBe(
      remote +
        "[L000003][P000001][@lewis][20260317T120100Z] local one\n" +
        "[L000004][P000003][@lewis][20260317T120200Z] local two\n",
    );
  });

  it("builds a per-author rebase commit message grouping renumbered lines", () => {
    // Rust build_rebase_commit_msg groups lines per author into one line
    // (the old TS emitted one line per entry).
    const resolved = resolveConflicts(
      { "channels/general.thread": localAdditions },
      { "channels/general.thread": remote },
    );
    expect(resolved.commitMessage).toBe(
      "msg: @lewis -> general L000003 L000004(rebased)",
    );
  });

  it("falls back to a generic message when there are no additions", () => {
    const resolved = resolveConflicts({}, {});
    expect(resolved.commitMessage).toBe("msg: sync after rebase");
  });

  it("extractThreadAdditions keeps append-only diffing in TS", () => {
    const base = "[L000001][P000000][@alice][20260317T120000Z] base\n";
    const local =
      base + "[L000002][P000001][@lewis][20260317T120100Z] local\n";
    expect(extractThreadAdditions("channels/general.thread", local, base)).toBe(
      "[L000002][P000001][@lewis][20260317T120100Z] local\n",
    );
  });
});

describe("parseChannelMeta/parseUserMeta — lenient deserialize via wasm", () => {
  it("parses a full channel meta", () => {
    const meta = parseChannelMeta(
      [
        "display_name: General",
        "created_by: alice",
        "created_at: 20260317T120000Z",
        "introduction: Team chat",
        "members:",
        "  - alice",
        "  - lewis",
        "",
      ].join("\n"),
    ) as { display_name: string; members: string[] };
    expect(meta.display_name).toBe("General");
    expect(meta.members).toEqual(["alice", "lewis"]);
  });

  it("rejects a channel meta missing required fields", () => {
    // Mirrors the daemon's serde deserialize: created_by/created_at/
    // introduction are required (the old lenient TS parser accepted partials).
    expect(() =>
      parseChannelMeta("display_name: General\nmembers:\n  - alice\n"),
    ).toThrow();
  });

  it("parses a user meta and tolerates a missing optional labels field", () => {
    const meta = parseUserMeta(
      "display_name: Alice\nrole: member\nintroduction: hi\n",
    ) as { display_name: string; role: string };
    expect(meta.display_name).toBe("Alice");
    expect(meta.role).toBe("member");
  });
});
