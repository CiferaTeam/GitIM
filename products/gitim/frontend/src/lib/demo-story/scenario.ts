import type {
  DemoCommit,
  DemoEffect,
  DemoFrame,
  DemoScenario,
  DemoState,
} from "./types";

/**
 * Landing demo v2 scenario — 26 frames, three chapters.
 * Frame-by-frame source of truth: docs/plans/landing-demo/01-storyboard.md (v2).
 *
 * Context: the night before the v2.4 release, prod webhooks double-fire
 * (duplicate invoices). The owner spins up an incident team from chat.
 */

export function formatThreadLine(
  line: number,
  pointTo: number,
  author: string,
  timestamp: string,
  body: string,
): string {
  const width = Math.max(String(line).length, 6);
  const linePad = String(line).padStart(width, "0");
  const pointPad = String(pointTo).padStart(width, "0");
  return `[L${linePad}][P${pointPad}][@${author}][${timestamp}] ${body}\n`;
}

/**
 * Narrated frames: clamp(3500, 2500 + 65 × wordCount, 7000).
 * Beat frames without narration: 2000ms (handled by `frame()` below).
 */
export function narrationDelayMs(narration: string): number {
  const words = narration
    .split(/\s+/)
    .filter((token) => /[A-Za-z0-9]/.test(token)).length;
  return Math.min(7000, Math.max(3500, 2500 + 65 * words));
}

// ---------------------------------------------------------------------------
// Anchors (data-anchor ids on stage elements; `git:` uses full repo paths)
// ---------------------------------------------------------------------------

const A_INPUT = "chat-input";
const A_MSG = (n: number) => `chat-msg-${n}`;
const A_MEMBER = (h: string) => `member-${h}`;
const A_CARD = (id: string) => `card-${id}`;
const A_THREAD = "git:channels/release-v2-4.thread";
const A_LATEST_COMMIT = "git:latest-commit";
const A_INV_YAML = "git:users/investigator.meta.yaml";
const A_FIX_YAML = "git:users/fixer.meta.yaml";
const A_CARD91_DIR = "git:channels/release-v2-4/cards/wh-3a91";
const A_CARD92_DIR = "git:channels/release-v2-4/cards/wh-3a92";
const A_CARD91_META = "git:channels/release-v2-4/cards/wh-3a91/card.meta.yaml";
const A_CARD91_DISC =
  "git:channels/release-v2-4/cards/wh-3a91/discussion.thread";
const A_CARD92_META = "git:channels/release-v2-4/cards/wh-3a92/card.meta.yaml";
const A_CARD92_DISC =
  "git:channels/release-v2-4/cards/wh-3a92/discussion.thread";

const arrow = (from: string, to: string, label?: string): DemoEffect => ({
  kind: "arrow",
  from,
  to,
  ...(label ? { label } : {}),
});
const pulse = (target: string): DemoEffect => ({ kind: "pulse", target });
const badge = (target: string, text: string): DemoEffect => ({
  kind: "badge",
  target,
  text,
});

// ---------------------------------------------------------------------------
// Timestamps (fictional: 2026-07-13, the night before the release, 21:4x UTC)
// ---------------------------------------------------------------------------

const TS_HISTORY_1 = "20260713T212000Z";
const TS_HISTORY_2 = "20260713T212100Z";
const TS_MSG_1 = "20260713T214000Z";
const TS_MSG_2 = "20260713T214100Z";
const TS_MSG_3 = "20260713T214200Z";
const TS_MSG_4 = "20260713T214300Z";
const TS_MSG_5 = "20260713T214400Z";
const TS_MSG_6 = "20260713T214500Z";
const TS_CARD91_MSG_1 = "20260713T214600Z";
const TS_CARD91_MSG_2 = "20260713T214700Z";
const TS_CARD92_MSG_1 = "20260713T214800Z";
const TS_MSG_7 = "20260713T214900Z";
const TS_MSG_8 = "20260713T215000Z";
const TS_MSG_9 = "20260713T215100Z";

// ---------------------------------------------------------------------------
// Message bodies (verbatim from the storyboard)
// ---------------------------------------------------------------------------

const BODY_1 = "v2.4 cuts tomorrow 10:00. Freeze starts tonight.";
const BODY_2 = "Noted — I'll keep this channel as the release log.";
const BODY_3 =
  "<@coordinator> prod is double-firing webhook retries — customers are getting duplicate invoices. v2.4 can't ship like this. Build me an incident team.";
const BODY_4 = "On it — spinning up two agents.";
const BODY_5 = "Cards up:";
const BODY_6 =
  "<@investigator> you're on wh-3a91, <@fixer> on wh-3a92. Findings go in the card threads.";
const BODY_CARD91_1 =
  "Found it. We ack before the dedupe check — anything retried inside the 30s window gets processed twice. Under load, that is every retry.";
const BODY_CARD91_2 =
  "Done on my side. <@fixer>: dedupe must run before ack, keyed on delivery id.";
const BODY_CARD92_1 =
  "Patch in PR #417: dedupe moved ahead of ack, idempotency keyed on delivery id. Canary clean for 30 min.";
const BODY_7 =
  "Both cards closed. Duplicate invoices at zero on canary — v2.4 is unblocked. Full trail is in Git.";
const BODY_8 =
  "Root cause confirmed: ack fired before the dedupe check, so every retry inside the 30s window ran twice. Full analysis is in wh-3a91.";
const BODY_9 =
  "Fix merged from PR #417 — dedupe now runs ahead of ack, keyed on delivery id. Canary's been clean for 30 minutes.";

const CHIP_ADD_INVESTIGATOR =
  "gitim-runtime add-agent --handler investigator --provider claude";
const CHIP_ADD_FIXER =
  "gitim-runtime add-agent --handler fixer --provider codex";
const CHIP_CREATE_CARD_1 =
  "gitim card create release-v2-4 'Investigate duplicate webhook retries' --assignee investigator --label incident";
const CHIP_CREATE_CARD_2 =
  "gitim card create release-v2-4 'Patch retry dedupe' --assignee fixer --label incident";

// ---------------------------------------------------------------------------
// File contents
// ---------------------------------------------------------------------------

const lewisMetaYaml = `display_name: Lewis
role: owner
introduction: Workspace owner. Authorizes organizational changes in natural language.
labels: []
`;

const coordinatorMetaYaml = `display_name: Coordinator
role: coordinator
introduction: Visible workspace agent that turns human intent into GitIM CLI actions.
labels: []
`;

const investigatorMetaYaml = `display_name: Investigator
role: investigator
introduction: Incident investigator. Digs through logs and finds the root cause.
labels: []
`;

const fixerMetaYaml = `display_name: Fixer
role: fixer
introduction: Ships minimal, reviewable patches under incident pressure.
labels: []
`;

const channelMetaYaml = `display_name: Release v2.4
created_by: lewis
created_at: 20260713T213000Z
introduction: Release coordination for v2.4 — cuts tomorrow 10:00.
members:
  - lewis
  - coordinator
`;

const threadInitial =
  formatThreadLine(1, 0, "lewis", TS_MSG_1, BODY_1) +
  formatThreadLine(2, 0, "coordinator", TS_MSG_2, BODY_2);
const threadAfter3 = threadInitial + formatThreadLine(3, 0, "lewis", TS_MSG_3, BODY_3);
const threadAfter4 = threadAfter3 + formatThreadLine(4, 0, "coordinator", TS_MSG_4, BODY_4);
const threadAfter5 = threadAfter4 + formatThreadLine(5, 0, "coordinator", TS_MSG_5, BODY_5);
const threadAfter6 = threadAfter5 + formatThreadLine(6, 0, "coordinator", TS_MSG_6, BODY_6);
const threadAfter7 = threadAfter6 + formatThreadLine(7, 0, "coordinator", TS_MSG_7, BODY_7);
const threadAfter8 = threadAfter7 + formatThreadLine(8, 0, "investigator", TS_MSG_8, BODY_8);
const threadAfter9 = threadAfter8 + formatThreadLine(9, 0, "fixer", TS_MSG_9, BODY_9);

function cardMetaYaml(
  title: string,
  status: "todo" | "doing" | "done",
  assignee: string,
  createdAt: string,
  updatedAt: string,
): string {
  return `title: ${title}
channel: release-v2-4
status: ${status}
labels:
  - incident
assignee: ${assignee}
created_by: coordinator
created_at: ${createdAt}
updated_at: ${updatedAt}
`;
}

const CARD91_TITLE = "Investigate duplicate webhook retries";
const CARD92_TITLE = "Patch retry dedupe";

const card91MetaTodo = cardMetaYaml(CARD91_TITLE, "todo", "investigator", TS_MSG_5, TS_MSG_5);
const card91MetaDoing = cardMetaYaml(CARD91_TITLE, "doing", "investigator", TS_MSG_5, TS_CARD91_MSG_1);
const card91MetaDone = cardMetaYaml(CARD91_TITLE, "done", "investigator", TS_MSG_5, TS_CARD91_MSG_2);
const card92MetaTodo = cardMetaYaml(CARD92_TITLE, "todo", "fixer", TS_MSG_5, TS_MSG_5);
const card92MetaDone = cardMetaYaml(CARD92_TITLE, "done", "fixer", TS_MSG_5, TS_CARD92_MSG_1);

const card91DiscAfter1 = formatThreadLine(1, 0, "investigator", TS_CARD91_MSG_1, BODY_CARD91_1);
const card91DiscAfter2 =
  card91DiscAfter1 + formatThreadLine(2, 1, "investigator", TS_CARD91_MSG_2, BODY_CARD91_2);
const card92DiscAfter1 = formatThreadLine(1, 0, "fixer", TS_CARD92_MSG_1, BODY_CARD92_1);

// ---------------------------------------------------------------------------
// Commits (message strings verbatim from the storyboard's 16-commit list)
// ---------------------------------------------------------------------------

const commit = (
  id: string,
  message: string,
  author: string,
  timestamp: string,
): DemoCommit => ({ id, message, author, timestamp });

const C1 = commit("a3f19c2", "msg: @lewis -> release-v2-4 L000003", "lewis", TS_MSG_3);
const C2 = commit("b58e2d4", "msg: @coordinator -> release-v2-4 L000004", "coordinator", TS_MSG_4);
const C3 = commit("c6a04e8", "user: register @investigator", "investigator", TS_MSG_4);
const C4 = commit("d91b7f3", "user: register @fixer", "fixer", TS_MSG_4);
const C5 = commit("e2c48a6", "msg: @coordinator -> release-v2-4 L000005", "coordinator", TS_MSG_5);
const C6 = commit("f75d3b9", "card: create wh-3a91 in release-v2-4 by @coordinator", "coordinator", TS_MSG_5);
const C7 = commit("084e6c1", "card: create wh-3a92 in release-v2-4 by @coordinator", "coordinator", TS_MSG_5);
const C8 = commit("19af85d", "msg: @coordinator -> release-v2-4 L000006", "coordinator", TS_MSG_6);
const C9 = commit("2ab69e0", "card: update wh-3a91 in release-v2-4 by @investigator", "investigator", TS_CARD91_MSG_1);
const C10 = commit("3c1d7a5", "msg: @investigator -> release-v2-4/wh-3a91 L000001", "investigator", TS_CARD91_MSG_1);
const C11 = commit("4d8b2f7", "msg: @investigator -> release-v2-4/wh-3a91 L000002", "investigator", TS_CARD91_MSG_2);
const C12 = commit("5e94c08", "card: update wh-3a91 in release-v2-4 by @investigator", "investigator", TS_CARD91_MSG_2);
const C13 = commit("6f0a5d3", "card: update wh-3a92 in release-v2-4 by @fixer", "fixer", TS_CARD92_MSG_1);
const C14 = commit("70b3e9c", "msg: @fixer -> release-v2-4/wh-3a92 L000001", "fixer", TS_CARD92_MSG_1);
const C15 = commit("81c6f2b", "card: update wh-3a92 in release-v2-4 by @fixer", "fixer", TS_CARD92_MSG_1);
const C16 = commit("92d8a4e", "msg: @coordinator -> release-v2-4 L000007", "coordinator", TS_MSG_7);
const C17 = commit("a5e31c9", "msg: @investigator -> release-v2-4 L000008", "investigator", TS_MSG_8);
const C18 = commit("b7f26d1", "msg: @fixer -> release-v2-4 L000009", "fixer", TS_MSG_9);

// ---------------------------------------------------------------------------
// Members / cards
// ---------------------------------------------------------------------------

const lewisMember = {
  handler: "lewis",
  displayName: "Lewis",
  provider: null,
  kind: "human" as const,
  status: "active" as const,
};
const coordinatorActive = {
  handler: "coordinator",
  displayName: "Coordinator",
  provider: null,
  kind: "agent" as const,
  status: "active" as const,
};
const coordinatorWorking = { ...coordinatorActive, status: "working" as const };
const investigatorActive = {
  handler: "investigator",
  displayName: "Investigator",
  provider: "claude",
  kind: "agent" as const,
  status: "active" as const,
};
const investigatorWorking = { ...investigatorActive, status: "working" as const };
const fixerActive = {
  handler: "fixer",
  displayName: "Fixer",
  provider: "codex",
  kind: "agent" as const,
  status: "active" as const,
};
const fixerWorking = { ...fixerActive, status: "working" as const };

const card91Todo = {
  cardId: "wh-3a91",
  title: CARD91_TITLE,
  status: "todo" as const,
  assignee: "investigator",
  labels: ["incident"],
};
const card91Doing = { ...card91Todo, status: "doing" as const };
const card91Done = { ...card91Todo, status: "done" as const };
const card92Todo = {
  cardId: "wh-3a92",
  title: CARD92_TITLE,
  status: "todo" as const,
  assignee: "fixer",
  labels: ["incident"],
};
const card92Done = { ...card92Todo, status: "done" as const };

// ---------------------------------------------------------------------------
// Messages
// ---------------------------------------------------------------------------

const msg1 = { lineNumber: 1, author: "lewis", body: BODY_1, timestamp: TS_MSG_1 };
const msg2 = { lineNumber: 2, author: "coordinator", body: BODY_2, timestamp: TS_MSG_2 };
const msg3 = { lineNumber: 3, author: "lewis", body: BODY_3, timestamp: TS_MSG_3 };
const msg4Chip1 = {
  lineNumber: 4,
  author: "coordinator",
  body: BODY_4,
  timestamp: TS_MSG_4,
  commandChips: [CHIP_ADD_INVESTIGATOR],
};
const msg4Chip2 = { ...msg4Chip1, commandChips: [CHIP_ADD_INVESTIGATOR, CHIP_ADD_FIXER] };
const msg5Chip1 = {
  lineNumber: 5,
  author: "coordinator",
  body: BODY_5,
  timestamp: TS_MSG_5,
  commandChips: [CHIP_CREATE_CARD_1],
};
const msg5Chip2 = { ...msg5Chip1, commandChips: [CHIP_CREATE_CARD_1, CHIP_CREATE_CARD_2] };
const msg6 = { lineNumber: 6, author: "coordinator", body: BODY_6, timestamp: TS_MSG_6 };
const msg7 = { lineNumber: 7, author: "coordinator", body: BODY_7, timestamp: TS_MSG_7 };
const msg8 = { lineNumber: 8, author: "investigator", body: BODY_8, timestamp: TS_MSG_8 };
const msg9 = { lineNumber: 9, author: "fixer", body: BODY_9, timestamp: TS_MSG_9 };

const card91Msg1 = {
  lineNumber: 1,
  author: "investigator",
  body: BODY_CARD91_1,
  timestamp: TS_CARD91_MSG_1,
  anchor: "card-msg-1",
};
const card91Msg2 = {
  lineNumber: 2,
  author: "investigator",
  body: BODY_CARD91_2,
  timestamp: TS_CARD91_MSG_2,
  anchor: "card-msg-2",
};
const card92Msg1 = {
  lineNumber: 1,
  author: "fixer",
  body: BODY_CARD92_1,
  timestamp: TS_CARD92_MSG_1,
  anchor: "card-msg-3",
};

// ---------------------------------------------------------------------------
// Frame builder
// ---------------------------------------------------------------------------

type FrameSpec = Omit<DemoFrame, "delayMs"> & { delayMs?: number };

function frame(spec: FrameSpec): DemoFrame {
  const delayMs =
    spec.delayMs ?? (spec.narration ? narrationDelayMs(spec.narration) : 2000);
  return { ...spec, delayMs };
}

// ---------------------------------------------------------------------------
// The 26 frames
// ---------------------------------------------------------------------------

export const incidentScenario: DemoScenario = {
  id: "incident-team-ppt-v2",
  title: "Spin up an incident team from chat",
  chapters: [
    { id: "incident", label: "Incident" },
    { id: "teamup", label: "Team up" },
    { id: "delivery", label: "Delivery" },
  ],
  initialState: buildInitialState(),
  frames: [
    // ------------------------------------------------------------ Chapter 1
    frame({
      id: "incident-opening",
      chapter: "incident",
      view: { kind: "channel" },
      title: "The night before v2.4",
      caption:
        "A GitIM workspace, the night before a release. One human, one coordinator.",
      narration:
        "A GitIM workspace, the night before a release. One human, one coordinator.",
      fileChanges: [],
      uiChanges: [],
    }),
    frame({
      id: "incident-typing",
      chapter: "incident",
      view: { kind: "channel" },
      title: "Production breaks",
      caption:
        "Then production breaks. You describe what you need — in plain language.",
      narration:
        "Then production breaks. You describe what you need — in plain language.",
      effects: [pulse(A_INPUT)],
      typing: { anchor: A_INPUT, text: BODY_3, cps: 55 },
      fileChanges: [],
      uiChanges: [],
    }),
    frame({
      id: "incident-send",
      chapter: "incident",
      view: { kind: "channel" },
      title: "Send the instruction",
      caption: "Hit send.",
      narration: "Hit send.",
      effects: [pulse(A_MSG(3))],
      fileChanges: [],
      uiChanges: [{ type: "channel-message", message: msg3 }],
    }),
    frame({
      id: "incident-commit",
      chapter: "incident",
      view: { kind: "channel" },
      title: "Every word lands in Git",
      caption:
        "Every word lands in a file — and a commit — the second it's sent.",
      narration:
        "Every word lands in a file — and a commit — the second it's sent.",
      effects: [
        arrow(A_MSG(3), A_THREAD),
        badge(A_THREAD, "+1 line"),
        arrow(A_MSG(3), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter3 },
      ],
      uiChanges: [],
      commits: [C1],
    }),
    frame({
      id: "incident-wake",
      chapter: "incident",
      view: { kind: "channel" },
      title: "The mention wakes the coordinator",
      caption: "The mention wakes the coordinator. Nobody else.",
      narration: "The mention wakes the coordinator. Nobody else.",
      effects: [pulse(A_MEMBER("coordinator")), badge(A_MEMBER("coordinator"), "working")],
      fileChanges: [],
      uiChanges: [{ type: "member", member: coordinatorWorking }],
    }),

    // ------------------------------------------------------------ Chapter 2
    frame({
      id: "teamup-reply",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Intent becomes CLI calls",
      caption: "The coordinator turns intent into CLI calls.",
      narration: "The coordinator turns intent into CLI calls.",
      effects: [pulse(A_MSG(4))],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter4 },
      ],
      uiChanges: [{ type: "channel-message", message: msg4Chip1 }],
      commits: [C2],
    }),
    frame({
      id: "teamup-investigator",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "A new teammate joins",
      caption: "A new teammate — running on Claude.",
      narration: "A new teammate — running on Claude.",
      effects: [
        arrow(A_MSG(4), A_MEMBER("investigator")),
        badge(A_MEMBER("investigator"), "new member · claude"),
      ],
      fileChanges: [],
      uiChanges: [{ type: "member", member: investigatorActive }],
    }),
    frame({
      id: "teamup-investigator-file",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Identity is a YAML file",
      caption:
        "Its identity is a YAML file — written by the daemon, never by hand.",
      narration:
        "Its identity is a YAML file — written by the daemon, never by hand.",
      effects: [
        arrow(A_MSG(4), A_INV_YAML),
        badge(A_INV_YAML, "new file"),
        arrow(A_MSG(4), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "users/investigator.meta.yaml", type: "add", content: investigatorMetaYaml },
      ],
      uiChanges: [],
      commits: [C3],
    }),
    frame({
      id: "teamup-second-command",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Second agent, second command",
      caption: "The coordinator issues the next CLI call.",
      effects: [pulse("chat-msg-4-chip-2")],
      fileChanges: [],
      uiChanges: [{ type: "channel-message", message: msg4Chip2 }],
    }),
    frame({
      id: "teamup-fixer",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "A different model for a different job",
      caption: "Different job, different model. Providers are per-agent.",
      narration: "Different job, different model. Providers are per-agent.",
      effects: [
        arrow(A_MSG(4), A_MEMBER("fixer")),
        badge(A_MEMBER("fixer"), "new member · codex"),
      ],
      fileChanges: [],
      uiChanges: [{ type: "member", member: fixerActive }],
    }),
    frame({
      id: "teamup-fixer-file",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Fixer identity committed",
      caption: "Another identity file, another commit.",
      effects: [
        arrow(A_MSG(4), A_FIX_YAML),
        badge(A_FIX_YAML, "new file"),
        arrow(A_MSG(4), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "users/fixer.meta.yaml", type: "add", content: fixerMetaYaml },
      ],
      uiChanges: [],
      commits: [C4],
    }),
    frame({
      id: "teamup-cards-up",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "The work becomes cards",
      caption: "Then the work becomes cards.",
      narration: "Then the work becomes cards.",
      effects: [
        pulse(A_MSG(5)),
        arrow(A_MSG(5), A_THREAD),
        badge(A_THREAD, "+1 line"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter5 },
      ],
      uiChanges: [{ type: "channel-message", message: msg5Chip1 }],
      commits: [C5],
    }),
    frame({
      id: "teamup-card-one",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Owned from the first second",
      caption: "Owned from the first second.",
      narration: "Owned from the first second.",
      effects: [
        arrow(A_MSG(5), A_CARD("wh-3a91")),
        badge(A_CARD("wh-3a91"), "new card · → investigator"),
      ],
      fileChanges: [],
      uiChanges: [{ type: "card", card: card91Todo }],
    }),
    frame({
      id: "teamup-card-one-files",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "A card is two small files",
      caption: "A card is two small files: metadata, and a discussion thread.",
      narration: "A card is two small files: metadata, and a discussion thread.",
      effects: [
        arrow(A_MSG(5), A_CARD91_DIR),
        badge(A_CARD91_DIR, "new file"),
        badge(A_CARD91_DIR, "new file"),
        arrow(A_MSG(5), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a91/card.meta.yaml", type: "add", content: card91MetaTodo },
        { path: "channels/release-v2-4/cards/wh-3a91/discussion.thread", type: "add", content: "" },
      ],
      uiChanges: [],
      commits: [C6],
    }),
    frame({
      id: "teamup-card-two",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Second card created",
      caption: "wh-3a92 goes to the fixer.",
      effects: [
        arrow(A_MSG(5), A_CARD("wh-3a92")),
        badge(A_CARD("wh-3a92"), "new card · → fixer"),
      ],
      fileChanges: [],
      uiChanges: [
        { type: "channel-message", message: msg5Chip2 },
        { type: "card", card: card92Todo },
      ],
    }),
    frame({
      id: "teamup-card-two-files",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Second card lands in Git",
      caption: "Metadata and discussion thread, committed.",
      effects: [
        arrow(A_MSG(5), A_CARD92_DIR),
        badge(A_CARD92_DIR, "new file"),
        badge(A_CARD92_DIR, "new file"),
        arrow(A_MSG(5), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a92/card.meta.yaml", type: "add", content: card92MetaTodo },
        { path: "channels/release-v2-4/cards/wh-3a92/discussion.thread", type: "add", content: "" },
      ],
      uiChanges: [],
      commits: [C7],
    }),
    frame({
      id: "teamup-assign",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Mentions route the work",
      caption:
        "Mentions route the work — each agent wakes only for its own card.",
      narration:
        "Mentions route the work — each agent wakes only for its own card.",
      effects: [
        pulse(A_MSG(6)),
        arrow(A_MSG(6), A_CARD("wh-3a91")),
        pulse(A_CARD("wh-3a91")),
        arrow(A_MSG(6), A_CARD("wh-3a92")),
        pulse(A_CARD("wh-3a92")),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter6 },
      ],
      uiChanges: [{ type: "channel-message", message: msg6 }],
      commits: [C8],
    }),
    frame({
      id: "teamup-working",
      chapter: "teamup",
      view: { kind: "channel" },
      title: "Both agents start working",
      caption: "investigator and fixer wake on their mentions.",
      effects: [
        pulse(A_MEMBER("investigator")),
        badge(A_MEMBER("investigator"), "working"),
        pulse(A_MEMBER("fixer")),
        badge(A_MEMBER("fixer"), "working"),
      ],
      fileChanges: [],
      uiChanges: [
        { type: "member", member: investigatorWorking },
        { type: "member", member: fixerWorking },
      ],
    }),

    // ------------------------------------------------------------ Chapter 3
    frame({
      id: "delivery-claim",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a91" },
      title: "Claim the work",
      caption: "First move: claim the work. A status flip is just a field edit.",
      narration:
        "First move: claim the work. A status flip is just a field edit.",
      effects: [
        arrow(A_MEMBER("investigator"), A_CARD("wh-3a91")),
        badge(A_CARD("wh-3a91"), "status → doing"),
        arrow(A_MEMBER("investigator"), A_CARD91_META),
        badge(A_CARD91_META, "~1 line"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a91/card.meta.yaml", type: "modify", content: card91MetaDoing },
      ],
      uiChanges: [{ type: "card", card: card91Doing }],
      commits: [C9],
    }),
    frame({
      id: "delivery-investigate",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a91" },
      title: "Investigation in the open",
      caption:
        "The investigation happens in the open — inside the card's own thread.",
      narration:
        "The investigation happens in the open — inside the card's own thread.",
      effects: [pulse("card-msg-1")],
      fileChanges: [],
      uiChanges: [{ type: "card-message", cardId: "wh-3a91", message: card91Msg1 }],
    }),
    frame({
      id: "delivery-investigate-commit",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a91" },
      title: "Findings land in Git",
      caption: "The discussion thread gains a line and a commit.",
      effects: [
        arrow("card-msg-1", A_CARD91_DISC),
        badge(A_CARD91_DISC, "+1 line"),
        arrow("card-msg-1", A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a91/discussion.thread", type: "modify", content: card91DiscAfter1 },
      ],
      uiChanges: [],
      commits: [C10],
    }),
    frame({
      id: "delivery-handoff",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a91" },
      title: "Handoff and closure",
      caption: "Findings, handoff, closure — all auditable.",
      narration: "Findings, handoff, closure — all auditable.",
      effects: [
        pulse("card-msg-2"),
        arrow("card-msg-2", A_CARD("wh-3a91")),
        badge(A_CARD("wh-3a91"), "status → done"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a91/discussion.thread", type: "modify", content: card91DiscAfter2 },
        { path: "channels/release-v2-4/cards/wh-3a91/card.meta.yaml", type: "modify", content: card91MetaDone },
      ],
      uiChanges: [
        { type: "card-message", cardId: "wh-3a91", message: card91Msg2 },
        { type: "card", card: card91Done },
        { type: "member", member: investigatorActive },
      ],
      commits: [C11, C12],
    }),
    frame({
      id: "delivery-fix",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a92" },
      title: "The fix ships",
      caption:
        "The fix ships — and the card says why, not just that it did.",
      narration:
        "The fix ships — and the card says why, not just that it did.",
      effects: [pulse("card-msg-3"), badge(A_CARD("wh-3a92"), "status → done")],
      fileChanges: [
        { path: "channels/release-v2-4/cards/wh-3a92/discussion.thread", type: "modify", content: card92DiscAfter1 },
        { path: "channels/release-v2-4/cards/wh-3a92/card.meta.yaml", type: "modify", content: card92MetaDone },
      ],
      uiChanges: [
        { type: "card-message", cardId: "wh-3a92", message: card92Msg1 },
        { type: "card", card: card92Done },
        { type: "member", member: fixerActive },
      ],
      commits: [C13, C14, C15],
    }),
    frame({
      id: "delivery-fix-commit",
      chapter: "delivery",
      view: { kind: "card", cardId: "wh-3a92" },
      title: "The fix lands in Git",
      caption: "Thread and metadata updated, in commits.",
      effects: [
        arrow("card-msg-3", A_CARD92_DISC),
        badge(A_CARD92_DISC, "+1 line"),
        arrow("card-msg-3", A_CARD92_META),
        badge(A_CARD92_META, "~1 line"),
      ],
      fileChanges: [],
      uiChanges: [],
    }),
    frame({
      id: "delivery-receipt",
      chapter: "delivery",
      view: { kind: "channel" },
      title: "Closing the loop",
      caption: "The coordinator closes the loop — in plain language.",
      narration: "The coordinator closes the loop — in plain language.",
      effects: [
        pulse(A_MSG(7)),
        arrow(A_MSG(7), A_THREAD),
        badge(A_THREAD, "+1 line"),
        arrow(A_MSG(7), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter7 },
      ],
      uiChanges: [
        { type: "channel-message", message: msg7 },
        { type: "member", member: coordinatorActive },
      ],
      commits: [C16],
    }),
    frame({
      id: "delivery-investigator-reports",
      chapter: "delivery",
      view: { kind: "channel" },
      title: "The investigator reports back",
      caption:
        "The agents report back — in the same channel, like any teammate.",
      narration:
        "The agents report back — in the same channel, like any teammate.",
      effects: [
        pulse(A_MSG(8)),
        arrow(A_MSG(8), A_MEMBER("investigator")),
        arrow(A_MSG(8), A_THREAD),
        badge(A_THREAD, "+1 line"),
        arrow(A_MSG(8), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter8 },
      ],
      uiChanges: [{ type: "channel-message", message: msg8 }],
      commits: [C17],
    }),
    frame({
      id: "delivery-fixer-reports",
      chapter: "delivery",
      view: { kind: "channel" },
      title: "The fixer reports back",
      caption:
        "Every voice in this channel — human or agent — is an actor in Git.",
      narration:
        "Every voice in this channel — human or agent — is an actor in Git.",
      effects: [
        pulse(A_MSG(9)),
        arrow(A_MSG(9), A_MEMBER("fixer")),
        arrow(A_MSG(9), A_THREAD),
        badge(A_THREAD, "+1 line"),
        arrow(A_MSG(9), A_LATEST_COMMIT),
        badge(A_LATEST_COMMIT, "new commit"),
      ],
      fileChanges: [
        { path: "channels/release-v2-4.thread", type: "modify", content: threadAfter9 },
      ],
      uiChanges: [{ type: "channel-message", message: msg9 }],
      commits: [C18],
    }),
    frame({
      id: "delivery-finale",
      chapter: "delivery",
      view: { kind: "channel" },
      title: "Audit complete",
      caption:
        "Two agents hired. Two cards closed. Twenty commits. You never left the chat.",
      narration:
        "Two agents hired. Two cards closed. Twenty commits. You never left the chat.",
      effects: [],
      fileChanges: [],
      uiChanges: [],
    }),
  ],
};

function buildInitialState(): DemoState {
  return {
    messages: {
      channel: [msg1, msg2],
      cards: {},
    },
    members: [lewisMember, coordinatorActive],
    cards: [],
    files: {
      "users/lewis.meta.yaml": {
        path: "users/lewis.meta.yaml",
        content: lewisMetaYaml,
        status: "unchanged",
      },
      "users/coordinator.meta.yaml": {
        path: "users/coordinator.meta.yaml",
        content: coordinatorMetaYaml,
        status: "unchanged",
      },
      "channels/release-v2-4.meta.yaml": {
        path: "channels/release-v2-4.meta.yaml",
        content: channelMetaYaml,
        status: "unchanged",
      },
      "channels/release-v2-4.thread": {
        path: "channels/release-v2-4.thread",
        content: threadInitial,
        status: "unchanged",
      },
    },
    commits: [
      commit("1e4a9b0", "user: register @lewis", "lewis", TS_HISTORY_1),
      commit("2b7c3d1", "user: register @coordinator", "coordinator", TS_HISTORY_2),
    ],
  };
}

// ---------------------------------------------------------------------------
// Pure state machine
// ---------------------------------------------------------------------------

export function stateAtFrame(
  scenario: DemoScenario,
  upToInclusive: number,
): DemoState {
  let state = cloneInitial(scenario.initialState);
  for (let i = 0; i <= upToInclusive && i < scenario.frames.length; i += 1) {
    state = applyFrame(state, scenario.frames[i]);
  }
  return state;
}

export function cloneInitial(initial: DemoState): DemoState {
  const files: DemoState["files"] = {};
  for (const [path, file] of Object.entries(initial.files)) {
    files[path] = { ...file, status: "unchanged" };
  }
  const cardMessages: DemoState["messages"]["cards"] = {};
  for (const [cardId, msgs] of Object.entries(initial.messages.cards)) {
    cardMessages[cardId] = msgs.map((m) => ({
      ...m,
      commandChips: m.commandChips ? [...m.commandChips] : undefined,
    }));
  }
  return {
    messages: {
      channel: initial.messages.channel.map((m) => ({
        ...m,
        commandChips: m.commandChips ? [...m.commandChips] : undefined,
      })),
      cards: cardMessages,
    },
    members: initial.members.map((m) => ({ ...m })),
    cards: initial.cards.map((c) => ({ ...c, labels: [...c.labels] })),
    files,
    commits: [...initial.commits],
  };
}

export function applyFrame(state: DemoState, frame: DemoFrame): DemoState {
  const nextFiles: DemoState["files"] = {};
  for (const [path, file] of Object.entries(state.files)) {
    nextFiles[path] = { ...file, status: "unchanged" };
  }
  for (const change of frame.fileChanges) {
    const existed = Object.prototype.hasOwnProperty.call(state.files, change.path);
    nextFiles[change.path] = {
      path: change.path,
      content: change.content,
      status: change.type === "add" && !existed ? "added" : "modified",
    };
  }

  const channel = state.messages.channel.map((m) => ({ ...m }));
  const cardThreads: DemoState["messages"]["cards"] = {};
  for (const [cardId, msgs] of Object.entries(state.messages.cards)) {
    cardThreads[cardId] = msgs.map((m) => ({ ...m }));
  }
  const members = state.members.map((m) => ({ ...m }));
  const cards = state.cards.map((c) => ({ ...c }));

  for (const ui of frame.uiChanges) {
    if (ui.type === "channel-message") {
      const idx = channel.findIndex((m) => m.lineNumber === ui.message.lineNumber);
      if (idx >= 0) {
        channel[idx] = ui.message;
      } else {
        channel.push(ui.message);
      }
    } else if (ui.type === "card-message") {
      const thread = cardThreads[ui.cardId] ?? [];
      const idx = thread.findIndex((m) => m.lineNumber === ui.message.lineNumber);
      if (idx >= 0) {
        thread[idx] = ui.message;
      } else {
        thread.push(ui.message);
      }
      cardThreads[ui.cardId] = thread;
    } else if (ui.type === "member") {
      const idx = members.findIndex((m) => m.handler === ui.member.handler);
      if (idx >= 0) {
        members[idx] = ui.member;
      } else {
        members.push(ui.member);
      }
    } else if (ui.type === "card") {
      const idx = cards.findIndex((c) => c.cardId === ui.card.cardId);
      if (idx >= 0) {
        cards[idx] = ui.card;
      } else {
        cards.push(ui.card);
      }
    }
  }

  return {
    messages: { channel, cards: cardThreads },
    members,
    cards,
    files: nextFiles,
    commits: frame.commits ? [...state.commits, ...frame.commits] : state.commits,
  };
}
