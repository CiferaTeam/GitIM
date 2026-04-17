import { describe, it, expect } from "vitest";
import { formatPollChanges, type PollChange } from "../context-manager.js";

describe("formatPollChanges", () => {
  const changes: PollChange[] = [
    {
      channel: "general",
      kind: "channel",
      entries: [
        { type: "message", author: "god", body: "天亮了。昨晚 frank 被杀害了。", line_number: 45, timestamp: "20260325T103000Z" },
        { type: "message", author: "alice", body: "太可惜了", line_number: 46, timestamp: "20260325T103015Z" },
        { type: "event", event_type: "join", author: "alice", line_number: 1, timestamp: "20260325T100000Z" },
      ],
    },
    {
      channel: "wolves",
      kind: "channel",
      entries: [
        { type: "message", author: "dave", body: "今晚杀谁？", line_number: 12, timestamp: "20260325T102500Z" },
      ],
    },
  ];

  it("formats messages grouped by channel, skips events", () => {
    const result = formatPollChanges(changes, "现在是白天讨论阶段，请发言。");
    expect(result).toContain("=== #general 新消息 (2条)");
    expect(result).toContain("[@god]");
    expect(result).toContain("天亮了");
    expect(result).toContain("=== #wolves 新消息 (1条)");
    expect(result).toContain("今晚杀谁");
    expect(result).toContain("=== 当前任务 ===");
    expect(result).not.toContain("join"); // events filtered out
  });

  it("skips channels with no messages (only events)", () => {
    const eventOnly: PollChange[] = [
      { channel: "general", kind: "channel", entries: [
        { type: "event", event_type: "join", author: "alice", line_number: 1, timestamp: "" },
      ]},
    ];
    const result = formatPollChanges(eventOnly, "任务");
    expect(result).not.toContain("#general");
    expect(result).toContain("=== 当前任务 ===");
  });

  it("formats DM channels correctly", () => {
    const dmChanges: PollChange[] = [
      { channel: "dm:alice,god", kind: "dm", entries: [
        { type: "message", author: "god", body: "你是预言家", line_number: 1, timestamp: "20260325T100000Z" },
      ]},
    ];
    const result = formatPollChanges(dmChanges, "任务");
    expect(result).toContain("=== DM(alice,god) 新消息");
  });

  it("includes thinking section when provided", () => {
    const result = formatPollChanges(
      changes,
      "任务",
      ["我怀疑 bob", "charlie 可疑"]
    );
    expect(result).toContain("=== 你的近期思考");
    expect(result).toContain("我怀疑 bob");
  });

  it("handles empty changes array", () => {
    const result = formatPollChanges([], "等待指示");
    expect(result).toContain("=== 当前任务 ===");
    expect(result).toContain("等待指示");
  });
});
