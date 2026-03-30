import { describe, it, expect } from "vitest";
import { Role, dmChannel } from "../types.js";
import { formatPollChanges, type PollChange } from "../context-manager.js";
import { makePlayerPrompt, GOD_SYSTEM_PROMPT } from "../prompts.js";

describe("E2E mock: full pipeline without LLM", () => {
  describe("poll changes → context injection", () => {
    it("wolf gets wolf channel messages in injection", () => {
      const changes: PollChange[] = [
        { channel: "general", kind: "channel", entries: [
          { type: "message", author: "god", body: "天黑了", line_number: 1, timestamp: "20260325T100000Z" },
        ]},
        { channel: "wolves", kind: "channel", entries: [
          { type: "message", author: "eve", body: "杀 alice", line_number: 1, timestamp: "20260325T100100Z" },
        ]},
      ];

      const injection = formatPollChanges(changes, "请在狼人频道讨论击杀目标。", ["alice 可能是预言家"]);
      expect(injection).toContain("#general");
      expect(injection).toContain("#wolves");
      expect(injection).toContain("杀 alice");
      expect(injection).toContain("alice 可能是预言家");
    });

    it("villager injection has no wolf channel (daemon filters)", () => {
      // Daemon would not include wolves channel for a villager.
      // Simulate: only general channel in poll result.
      const changes: PollChange[] = [
        { channel: "general", kind: "channel", entries: [
          { type: "message", author: "god", body: "天亮了", line_number: 2, timestamp: "20260325T100200Z" },
        ]},
      ];
      const injection = formatPollChanges(changes, "请发言。");
      expect(injection).not.toContain("wolves");
      expect(injection).toContain("#general");
    });

    it("DM channels format correctly in injection", () => {
      const dm = dmChannel("alice", "god");
      expect(dm).toBe("dm:alice,god");

      const changes: PollChange[] = [
        { channel: dm, kind: "dm", entries: [
          { type: "message", author: "god", body: "你是预言家", line_number: 1, timestamp: "" },
        ]},
      ];
      const injection = formatPollChanges(changes, "回复上帝");
      expect(injection).toContain("DM(alice,god)");
    });
  });

  describe("prompts", () => {
    it("God prompt contains game rules", () => {
      expect(GOD_SYSTEM_PROMPT).toContain("狼人杀");
      expect(GOD_SYSTEM_PROMPT).toContain("夜晚");
      expect(GOD_SYSTEM_PROMPT).toContain("【游戏结束】");
    });

    it("wolf player prompt includes partner info", () => {
      const prompt = makePlayerPrompt({
        handler: "dave", role: Role.Wolf,
        personality: "你很狡猾。", wolfPartners: ["eve"],
      });
      expect(prompt).toContain("eve");
      expect(prompt).toContain("狼人");
    });

    it("seer player prompt includes verify ability", () => {
      const prompt = makePlayerPrompt({
        handler: "alice", role: Role.Seer, personality: "你很聪明。",
      });
      expect(prompt).toContain("预言家");
    });

    it("villager prompt mentions voting", () => {
      const prompt = makePlayerPrompt({
        handler: "bob", role: Role.Villager, personality: "你很直率。",
      });
      expect(prompt).toContain("村民");
      expect(prompt).toContain("投票");
    });
  });
});
