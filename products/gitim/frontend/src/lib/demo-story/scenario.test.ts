import { describe, expect, it } from "vitest";
import {
  applyFrame,
  cloneInitial,
  formatThreadLine,
  incidentScenario,
  narrationDelayMs,
  stateAtFrame,
} from "./scenario";
import { buildFileTree } from "./tree";

/**
 * The 16 commits from docs/plans/landing-demo/01-storyboard.md, verbatim
 * and in canonical time order.
 */
const STORYBOARD_COMMITS = [
  "msg: @lewis -> release-v2-4 L000003",
  "msg: @coordinator -> release-v2-4 L000004",
  "user: register @investigator",
  "user: register @fixer",
  "msg: @coordinator -> release-v2-4 L000005",
  "card: create wh-3a91 in release-v2-4 by @coordinator",
  "card: create wh-3a92 in release-v2-4 by @coordinator",
  "msg: @coordinator -> release-v2-4 L000006",
  "card: update wh-3a91 in release-v2-4 by @investigator",
  "msg: @investigator -> release-v2-4/wh-3a91 L000001",
  "msg: @investigator -> release-v2-4/wh-3a91 L000002",
  "card: update wh-3a91 in release-v2-4 by @investigator",
  "card: update wh-3a92 in release-v2-4 by @fixer",
  "msg: @fixer -> release-v2-4/wh-3a92 L000001",
  "card: update wh-3a92 in release-v2-4 by @fixer",
  "msg: @coordinator -> release-v2-4 L000007",
  "msg: @investigator -> release-v2-4 L000008",
  "msg: @fixer -> release-v2-4 L000009",
];

const frameIndex = (id: string) =>
  incidentScenario.frames.findIndex((f) => f.id === id);

describe("formatThreadLine", () => {
  it("matches the six-digit gitim-core formatter prefix", () => {
    expect(
      formatThreadLine(
        1,
        0,
        "lewis",
        "20260713T214000Z",
        "<@coordinator> prod is broken",
      ),
    ).toBe(
      "[L000001][P000000][@lewis][20260713T214000Z] <@coordinator> prod is broken\n",
    );
  });
});

describe("narrationDelayMs", () => {
  it("clamps to [3500, 7000] around 2500 + 65ms per word", () => {
    expect(narrationDelayMs("Hit send.")).toBe(3500);
    // 20 words → 2500 + 1300 = 3800
    expect(
      narrationDelayMs("one two three four five six seven eight nine ten " +
        "eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen twenty"),
    ).toBe(3800);
    // very long narration clamps at 7000
    expect(narrationDelayMs(Array.from({ length: 200 }, (_, i) => `w${i}`).join(" "))).toBe(7000);
  });
});

describe("incidentScenario structure", () => {
  it("has exactly 28 frames in three chapters (5 / 13 / 10)", () => {
    expect(incidentScenario.frames).toHaveLength(28);
    const by = (ch: string) =>
      incidentScenario.frames.filter((f) => f.chapter === ch).length;
    expect(by("incident")).toBe(5);
    expect(by("teamup")).toBe(13);
    expect(by("delivery")).toBe(10);
    expect(incidentScenario.chapters.map((c) => c.id)).toEqual([
      "incident",
      "teamup",
      "delivery",
    ]);
  });

  it("keeps chapters contiguous and in order", () => {
    const chapters = incidentScenario.frames.map((f) => f.chapter);
    expect(chapters.slice(0, 5)).toEqual(Array(5).fill("incident"));
    expect(chapters.slice(5, 18)).toEqual(Array(13).fill("teamup"));
    expect(chapters.slice(18)).toEqual(Array(10).fill("delivery"));
  });

  it("uses 3500–7000ms for narrated frames and 2000ms for beat frames", () => {
    for (const f of incidentScenario.frames) {
      if (f.narration) {
        expect(f.delayMs).toBeGreaterThanOrEqual(3500);
        expect(f.delayMs).toBeLessThanOrEqual(7000);
        expect(f.delayMs).toBe(narrationDelayMs(f.narration));
      } else {
        expect(f.delayMs).toBe(2000);
      }
    }
  });

  it("carries narration text on exactly the frames the storyboard narrates", () => {
    const narrated = incidentScenario.frames
      .filter((f) => f.narration)
      .map((f) => f.id);
    expect(narrated).toEqual([
      "incident-opening",
      "incident-typing",
      "incident-send",
      "incident-commit",
      "incident-wake",
      "teamup-reply",
      "teamup-investigator",
      "teamup-investigator-file",
      "teamup-fixer",
      "teamup-cards-up",
      "teamup-card-one",
      "teamup-card-one-files",
      "teamup-assign",
      "delivery-claim",
      "delivery-investigate",
      "delivery-handoff",
      "delivery-fix",
      "delivery-receipt",
      "delivery-investigator-reports",
      "delivery-fixer-reports",
      "delivery-finale",
    ]);
  });

  it("ends with a fully lit finale frame (no effects)", () => {
    const last = incidentScenario.frames[incidentScenario.frames.length - 1];
    expect(last.id).toBe("delivery-finale");
    expect(last.effects).toEqual([]);
  });
});

describe("incidentScenario initial state (prefilled, no empty box)", () => {
  it("starts with two history messages, two members, no cards, two commits", () => {
    const initial = cloneInitial(incidentScenario.initialState);
    expect(initial.messages.channel).toHaveLength(2);
    expect(initial.messages.channel[0].author).toBe("lewis");
    expect(initial.messages.channel[1].author).toBe("coordinator");
    expect(initial.members.map((m) => m.handler)).toEqual([
      "lewis",
      "coordinator",
    ]);
    expect(initial.cards).toEqual([]);
    expect(initial.commits).toHaveLength(2);
    expect(Object.keys(initial.files).sort()).toEqual([
      "channels/release-v2-4.meta.yaml",
      "channels/release-v2-4.thread",
      "users/coordinator.meta.yaml",
      "users/lewis.meta.yaml",
    ]);
  });
});

describe("incidentScenario frame application", () => {
  it("wakes the coordinator at incident-wake and nobody else", () => {
    const state = stateAtFrame(incidentScenario, frameIndex("incident-wake"));
    expect(state.members.find((m) => m.handler === "coordinator")?.status).toBe(
      "working",
    );
    expect(state.members).toHaveLength(2);
  });

  it("adds investigator and fixer with per-agent providers", () => {
    const state = stateAtFrame(incidentScenario, frameIndex("teamup-fixer"));
    const inv = state.members.find((m) => m.handler === "investigator");
    const fix = state.members.find((m) => m.handler === "fixer");
    expect(inv?.provider).toBe("claude");
    expect(fix?.provider).toBe("codex");
  });

  it("grows coordinator command chips across frames", () => {
    const atReply = stateAtFrame(incidentScenario, frameIndex("teamup-reply"));
    expect(
      atReply.messages.channel.find((m) => m.lineNumber === 4)?.commandChips,
    ).toHaveLength(1);
    const atSecond = stateAtFrame(
      incidentScenario,
      frameIndex("teamup-second-command"),
    );
    const chips = atSecond.messages.channel.find((m) => m.lineNumber === 4)
      ?.commandChips;
    expect(chips).toHaveLength(2);
    expect(chips?.[1]).toBe(
      "gitim-runtime add-agent --handler fixer --provider codex",
    );
  });

  it("creates both cards assigned with todo status", () => {
    const state = stateAtFrame(incidentScenario, frameIndex("teamup-card-two"));
    expect(state.cards.map((c) => c.cardId)).toEqual(["wh-3a91", "wh-3a92"]);
    expect(state.cards[0]).toMatchObject({
      status: "todo",
      assignee: "investigator",
      labels: ["incident"],
    });
    expect(state.cards[1]).toMatchObject({ status: "todo", assignee: "fixer" });
  });

  it("flips wh-3a91 todo → doing → done across chapter 3", () => {
    const atClaim = stateAtFrame(incidentScenario, frameIndex("delivery-claim"));
    expect(atClaim.cards.find((c) => c.cardId === "wh-3a91")?.status).toBe(
      "doing",
    );
    const atHandoff = stateAtFrame(
      incidentScenario,
      frameIndex("delivery-handoff"),
    );
    expect(atHandoff.cards.find((c) => c.cardId === "wh-3a91")?.status).toBe(
      "done",
    );
  });

  it("returns agents to active when their cards close", () => {
    const atHandoff = stateAtFrame(
      incidentScenario,
      frameIndex("delivery-handoff"),
    );
    expect(
      atHandoff.members.find((m) => m.handler === "investigator")?.status,
    ).toBe("active");
    const atFix = stateAtFrame(incidentScenario, frameIndex("delivery-fix"));
    expect(atFix.members.find((m) => m.handler === "fixer")?.status).toBe(
      "active",
    );
    const final = stateAtFrame(incidentScenario, 27);
    expect(final.members.every((m) => m.status === "active")).toBe(true);
  });

  it("collects card discussion threads with global card-msg anchors", () => {
    const final = stateAtFrame(incidentScenario, 27);
    expect(final.messages.cards["wh-3a91"]).toHaveLength(2);
    expect(final.messages.cards["wh-3a92"]).toHaveLength(1);
    expect(final.messages.cards["wh-3a91"][0].anchor).toBe("card-msg-1");
    expect(final.messages.cards["wh-3a91"][1].anchor).toBe("card-msg-2");
    expect(final.messages.cards["wh-3a92"][0].anchor).toBe("card-msg-3");
  });

  it("touches exactly the seven files the storyboard lists", () => {
    const final = stateAtFrame(incidentScenario, 27);
    const touched = [
      "channels/release-v2-4.thread",
      "users/investigator.meta.yaml",
      "users/fixer.meta.yaml",
      "channels/release-v2-4/cards/wh-3a91/card.meta.yaml",
      "channels/release-v2-4/cards/wh-3a91/discussion.thread",
      "channels/release-v2-4/cards/wh-3a92/card.meta.yaml",
      "channels/release-v2-4/cards/wh-3a92/discussion.thread",
    ];
    for (const path of touched) {
      expect(final.files[path], path).toBeDefined();
    }
    expect(final.messages.channel).toHaveLength(9);
  });

  it("keeps applyFrame file status ephemeral across subsequent frames", () => {
    const first = applyFrame(
      cloneInitial(incidentScenario.initialState),
      incidentScenario.frames[0],
    );
    expect(first.files["channels/release-v2-4.thread"]?.status).toBe(
      "unchanged",
    );
    const commitIdx = frameIndex("incident-commit");
    const second = applyFrame(
      cloneInitial(incidentScenario.initialState),
      incidentScenario.frames[commitIdx],
    );
    expect(second.files["channels/release-v2-4.thread"]?.status).toBe(
      "modified",
    );
  });
});

describe("incidentScenario commit sequence", () => {
  it("reproduces the storyboard's 16 commits verbatim, in order", () => {
    const final = stateAtFrame(incidentScenario, 27);
    const messages = final.commits.map((c) => c.message);
    // Two prefilled history commits come first.
    expect(messages.slice(0, 2)).toEqual([
      "user: register @lewis",
      "user: register @coordinator",
    ]);
    expect(messages.slice(2)).toEqual(STORYBOARD_COMMITS);
    expect(final.commits).toHaveLength(20);
  });

  it("assigns unique commit ids", () => {
    const final = stateAtFrame(incidentScenario, 27);
    const ids = final.commits.map((c) => c.id);
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("buildFileTree", () => {
  it("nests channel card paths", () => {
    const tree = buildFileTree([
      "users/coordinator.meta.yaml",
      "channels/release-v2-4/cards/wh-3a91/card.meta.yaml",
    ]);
    const channels = tree.children.find((c) => c.name === "channels");
    expect(channels?.kind).toBe("directory");
    const release = channels?.children.find((c) => c.name === "release-v2-4");
    const cards = release?.children.find((c) => c.name === "cards");
    expect(
      cards?.children.find((c) => c.name === "wh-3a91")?.children[0]?.name,
    ).toBe("card.meta.yaml");
  });
});
