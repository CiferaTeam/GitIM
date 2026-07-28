/**
 * Landing demo v2 — data model.
 *
 * Single-scene stage (no tabs): chat main view + persistent
 * Members / Cards / Git sidebar. Causal highlight effects
 * (arrow / pulse / badge) replace the old single spotlight.
 * See docs/plans/landing-demo/00-design.md §5.
 */

export type DemoChapter = "incident" | "teamup" | "delivery";

/** Stable anchor id registered on stage elements via `data-anchor`. */
export type AnchorId = string;

export type DemoEffect =
  | { kind: "arrow"; from: AnchorId; to: AnchorId; label?: string }
  | { kind: "pulse"; target: AnchorId }
  | { kind: "badge"; target: AnchorId; text: string };

export interface DemoMember {
  handler: string;
  displayName: string;
  /** LLM provider for agents; null for humans / unspecified. */
  provider: string | null;
  kind: "human" | "agent";
  status: "active" | "working";
}

export interface DemoMessage {
  lineNumber: number;
  author: string;
  body: string;
  timestamp: string;
  /** Inline CLI chips rendered under coordinator messages. */
  commandChips?: string[];
  /**
   * Explicit anchor id. Channel messages default to `chat-msg-<lineNumber>`;
   * card discussion messages carry a global `card-msg-<n>` anchor.
   */
  anchor?: AnchorId;
}

export interface DemoCard {
  cardId: string;
  title: string;
  status: "todo" | "doing" | "done";
  assignee: string;
  labels: string[];
}

export interface DemoFile {
  path: string;
  content: string;
  status: "unchanged" | "added" | "modified";
}

export interface DemoCommit {
  id: string;
  message: string;
  author: string;
  timestamp: string;
}

export interface DemoState {
  /** Channel thread plus one discussion thread per card. */
  messages: {
    channel: DemoMessage[];
    cards: Record<string, DemoMessage[]>;
  };
  members: DemoMember[];
  cards: DemoCard[];
  files: Record<string, DemoFile>;
  commits: DemoCommit[];
}

export type DemoView = { kind: "channel" } | { kind: "card"; cardId: string };

export interface DemoTyping {
  anchor: AnchorId;
  text: string;
  /** Characters per second. Defaults to 50. */
  cps?: number;
}

export interface FileChange {
  path: string;
  type: "add" | "modify";
  content: string;
}

export type UiChange =
  | { type: "channel-message"; message: DemoMessage }
  | { type: "card-message"; cardId: string; message: DemoMessage }
  | { type: "member"; member: DemoMember }
  | { type: "card"; card: DemoCard };

export interface DemoFrame {
  id: string;
  chapter: DemoChapter;
  /** Fallback pacing when no narration audio exists yet. */
  delayMs: number;
  /** Main view: channel stream, or one card's discussion thread. */
  view: DemoView;
  /** Narration bar title. */
  title: string;
  /** Narration bar subtitle; doubles as future caption text. */
  caption?: string;
  /** English narration text — input for the offline TTS pipeline. */
  narration?: string;
  /** Causal highlight effects for this frame. */
  effects?: DemoEffect[];
  /** Typewriter animation in the chat input. */
  typing?: DemoTyping;
  fileChanges: FileChange[];
  uiChanges: UiChange[];
  /** One frame may produce several commits (kept in canonical time order). */
  commits?: DemoCommit[];
}

export interface DemoChapterInfo {
  id: DemoChapter;
  label: string;
}

export interface DemoScenario {
  id: string;
  title: string;
  chapters: DemoChapterInfo[];
  initialState: DemoState;
  frames: DemoFrame[];
}
