import { describe, it, expect } from "vitest";
import { Role, getVisibleChannels } from "../types.js";
import { formatInjection, ChannelMessages, maxLineFromMessages } from "../context-manager.js";
import { makePlayerPrompt, GOD_SYSTEM_PROMPT } from "../prompts.js";

describe("E2E mock: full pipeline without LLM", () => {
  const wolves = ["dave", "eve"];

  describe("role assignment → visibility → context", () => {
    it("wolf gets correct visibility and context injection", () => {
      const channels = getVisibleChannels("dave", Role.Wolf, wolves);
      expect(channels).toContain("wolves");
      expect(channels).toContain("general");

      const msgs: ChannelMessages = {
        general: [{ author: "god", body: "天黑了", line_number: 1, timestamp: "20260325T100000Z" }],
        wolves: [{ author: "eve", body: "杀 alice", line_number: 1, timestamp: "20260325T100100Z" }],
      };

      const injection = formatInjection(msgs, "请在狼人频道讨论击杀目标。", ["alice 可能是预言家"]);
      expect(injection).toContain("#general");
      expect(injection).toContain("#wolves");
      expect(injection).toContain("杀 alice");
      expect(injection).toContain("alice 可能是预言家");
    });

    it("villager cannot see wolf channel", () => {
      const channels = getVisibleChannels("bob", Role.Villager, wolves);
      expect(channels).not.toContain("wolves");

      const msgs: ChannelMessages = {
        general: [{ author: "god", body: "天亮了", line_number: 2, timestamp: "20260325T100200Z" }],
      };
      const injection = formatInjection(msgs, "请发言。");
      expect(injection).not.toContain("wolves");
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

  describe("cursor tracking", () => {
    it("maxLineFromMessages returns correct cursors", () => {
      const msgs: ChannelMessages = {
        general: [
          { author: "a", body: "hi", line_number: 3, timestamp: "" },
          { author: "b", body: "yo", line_number: 7, timestamp: "" },
        ],
        wolves: [{ author: "c", body: "kill", line_number: 2, timestamp: "" }],
      };
      const cursors = maxLineFromMessages(msgs);
      expect(cursors.general).toBe(7);
      expect(cursors.wolves).toBe(2);
    });
  });
});
