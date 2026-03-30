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
    it("God prompt contains setup phase and game rules", () => {
      expect(GOD_SYSTEM_PROMPT).toContain("第一阶段：游戏设置");
      expect(GOD_SYSTEM_PROMPT).toContain("第二阶段：游戏流程");
      expect(GOD_SYSTEM_PROMPT).toContain('回复"收到"');
      expect(GOD_SYSTEM_PROMPT).toContain("#wolves");
      expect(GOD_SYSTEM_PROMPT).toContain("join_channel");
      expect(GOD_SYSTEM_PROMPT).toContain("【游戏结束】");
    });

    it("player prompt is generic — no role information", () => {
      const prompt = makePlayerPrompt({
        handler: "alice",
        personality: "你很聪明。",
      });
      expect(prompt).toContain("@alice");
      expect(prompt).toContain("你很聪明");
      expect(prompt).toContain("还不知道自己的角色");
      expect(prompt).toContain("send_message");
      expect(prompt).not.toContain("你的身份");
      expect(prompt).not.toContain("预言家");
    });

    it("player prompt instructs to confirm role receipt", () => {
      const prompt = makePlayerPrompt({
        handler: "bob",
        personality: "你很直率。",
      });
      expect(prompt).toContain("收到");
      expect(prompt).toContain("@god");
    });
  });
});
