import { describe, it, expect } from "vitest";
import { dmChannel } from "../types.js";
import { formatPollChanges, type PollChange } from "../context-manager.js";
import { makePlayerSystemPrompt, makeGodSystemPrompt } from "../prompts.js";

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
    it("God prompt contains setup phase, game rules, and CLI tools", () => {
      const prompt = makeGodSystemPrompt(1);
      expect(prompt).toContain("第一阶段：游戏设置");
      expect(prompt).toContain("第二阶段：游戏流程");
      expect(prompt).toContain("【游戏结束】");
      expect(prompt).toContain("gitim send");
      expect(prompt).toContain("gitim dm send");
      expect(prompt).toContain("gitim create-channel");
      expect(prompt).toContain("通信机制");
      expect(prompt).toContain("werewolf-1");
    });

    it("player prompt contains handler, personality, CLI tools, and guidelines", () => {
      const prompt = makePlayerSystemPrompt({
        handler: "alice",
        personality: "你很聪明。",
        gameId: 1,
      });
      expect(prompt).toContain("@alice");
      expect(prompt).toContain("你很聪明");
      expect(prompt).toContain("gitim send");
      expect(prompt).toContain("gitim dm send");
      expect(prompt).toContain("-a alice");
      expect(prompt).toContain("通信机制");
      expect(prompt).toContain("不要主动读取");
    });

    it("player prompt does not contain god-only tools", () => {
      const prompt = makePlayerSystemPrompt({
        handler: "bob",
        personality: "你很直率。",
        gameId: 1,
      });
      expect(prompt).not.toContain("gitim create-channel");
    });

    it("god prompt forbids read but mentions it as prohibited", () => {
      const prompt = makeGodSystemPrompt(1);
      expect(prompt).toContain("不要主动读取");
      // God's tool list should not include read as an available command
      const toolSection = prompt.split("# 通信工具")[1]?.split("# ")[0] ?? "";
      expect(toolSection).not.toMatch(/^## 读取消息/m);
    });
  });
});
