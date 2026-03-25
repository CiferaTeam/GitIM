import { describe, it, expect } from "vitest";
import { formatInjection, ChannelMessages, maxLineFromMessages } from "../context-manager.js";

describe("formatInjection", () => {
  const channelMessages: ChannelMessages = {
    general: [
      { author: "god", body: "天亮了。昨晚 frank 被杀害了。", line_number: 45, timestamp: "20260325T103000Z" },
      { author: "alice", body: "太可惜了", line_number: 46, timestamp: "20260325T103015Z" },
    ],
    wolves: [
      { author: "dave", body: "今晚杀谁？", line_number: 12, timestamp: "20260325T102500Z" },
    ],
  };

  it("formats messages grouped by channel", () => {
    const result = formatInjection(channelMessages, "现在是白天讨论阶段，请发言。");
    expect(result).toContain("=== #general 新消息");
    expect(result).toContain("[@god]");
    expect(result).toContain("天亮了");
    expect(result).toContain("=== #wolves 新消息");
    expect(result).toContain("今晚杀谁");
    expect(result).toContain("=== 当前任务 ===");
    expect(result).toContain("请发言");
  });

  it("skips channels with no messages", () => {
    const result = formatInjection({ general: [] }, "任务");
    expect(result).not.toContain("#general");
    expect(result).toContain("=== 当前任务 ===");
  });

  it("includes thinking section when provided", () => {
    const result = formatInjection(
      { general: channelMessages.general },
      "任务",
      ["我怀疑 bob", "charlie 可疑"]
    );
    expect(result).toContain("=== 你的近期思考");
    expect(result).toContain("我怀疑 bob");
  });
});

describe("maxLineFromMessages", () => {
  it("returns correct cursors per channel", () => {
    const msgs: ChannelMessages = {
      general: [
        { author: "a", body: "hi", line_number: 3, timestamp: "" },
        { author: "b", body: "yo", line_number: 7, timestamp: "" },
      ],
      wolves: [
        { author: "c", body: "kill", line_number: 2, timestamp: "" },
      ],
    };
    const cursors = maxLineFromMessages(msgs);
    expect(cursors.general).toBe(7);
    expect(cursors.wolves).toBe(2);
  });
});
