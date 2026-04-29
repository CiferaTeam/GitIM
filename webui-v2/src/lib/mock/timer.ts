import type { Message, PollChange } from "../types";
import { nowTimestamp } from "../types";
import { getMockAgents, getMockMessages, pushChange } from "./client";

const MESSAGE_POOL: string[] = [
  "LGTM, merge when ready",
  "Can you take a look at the auth middleware?",
  "Found a race condition in the sync loop",
  "Deploying to staging now",
  "Tests are passing on CI",
  "I'll pick up the refactor after lunch",
  "The API response time improved by 40% after the index change",
  "Heads up: the dev server will be down for ~5 min for maintenance",
  "PR #51 is up — small change, should be quick to review",
  "Anyone else seeing the timeout errors in staging?",
  "Fixed. Was a missing await in the handler.",
  "The new mock timer is working nicely",
  "Rebasing on main before I open the PR",
  "Code coverage is at 78% — we should hit 80% this sprint",
  "Just merged the auth refactor. Please pull latest.",
  "Can we sync briefly on the polling interval? I think 500ms is too aggressive.",
  "All good on my end. Green across the board.",
  "Leaving a comment on the PR — minor naming nit, otherwise good",
  "The index rebuild finished. Query times are back to normal.",
  "Will handle after standup",
];

const MAX_MESSAGES_PER_CHANNEL = 500;

let messageTimerHandle: ReturnType<typeof setTimeout> | null = null;
let agentTimerHandle: ReturnType<typeof setTimeout> | null = null;

function rand(min: number, max: number): number {
  return Math.floor(Math.random() * (max - min + 1)) + min;
}

function pick<T>(arr: T[]): T {
  return arr[Math.floor(Math.random() * arr.length)];
}

function scheduleMessage(): void {
  const delay = rand(5000, 10000);
  messageTimerHandle = setTimeout(() => {
    injectMessage();
    scheduleMessage();
  }, delay);
}

function injectMessage(): void {
  const messages = getMockMessages();
  const channelKeys = Object.keys(messages);
  if (channelKeys.length === 0) return;

  const channelKey = pick(channelKeys);
  const channelMsgs = messages[channelKey];

  // Determine non-"lewis" authors for this channel. Fall back to any non-system author.
  // We derive potential authors from existing messages in the channel.
  const authorsInChannel = Array.from(
    new Set(
      channelMsgs
        .filter((m) => m.author !== "lewis" && m.author !== "system")
        .map((m) => m.author)
    )
  );

  if (authorsInChannel.length === 0) return;

  const author = pick(authorsInChannel);
  const body = pick(MESSAGE_POOL);

  // ~20% chance of a reply
  let point_to = 0;
  if (Math.random() < 0.2 && channelMsgs.length > 0) {
    const replyTarget = pick(channelMsgs);
    point_to = replyTarget.line_number;
  }

  const maxLine = channelMsgs.reduce(
    (m, msg) => Math.max(m, msg.line_number),
    0
  );
  const line_number = maxLine + 1;

  const msg: Message = {
    line_number,
    point_to,
    author,
    timestamp: nowTimestamp(),
    body,
  };

  channelMsgs.push(msg);

  // Cap at MAX_MESSAGES_PER_CHANNEL
  if (channelMsgs.length > MAX_MESSAGES_PER_CHANNEL) {
    channelMsgs.splice(0, channelMsgs.length - MAX_MESSAGES_PER_CHANNEL);
  }

  const change: PollChange = { channel: channelKey, kind: "message" };
  pushChange(change);
}

function scheduleAgentUpdate(): void {
  const delay = rand(15000, 30000);
  agentTimerHandle = setTimeout(() => {
    updateAgentStatus();
    scheduleAgentUpdate();
  }, delay);
}

function updateAgentStatus(): void {
  const agents = getMockAgents();
  if (agents.length === 0) return;

  const agent = pick(agents);

  if (agent.status === "running") {
    agent.status = "idle";
  } else if (agent.status === "idle") {
    agent.status = "running";
  } else {
    // error or offline → running (simulating recovery)
    agent.status = "running";
  }

  agent.lastActivity = new Date().toISOString();
  agent.messagesProcessed += rand(1, 5);
}

export function startMockTimer(): void {
  // Idempotent guard — stop any running timers before starting new ones.
  // Prevents duplicate timers when React StrictMode mounts effects twice.
  stopMockTimer();
  scheduleMessage();
  scheduleAgentUpdate();
}

export function stopMockTimer(): void {
  if (messageTimerHandle !== null) {
    clearTimeout(messageTimerHandle);
    messageTimerHandle = null;
  }
  if (agentTimerHandle !== null) {
    clearTimeout(agentTimerHandle);
    agentTimerHandle = null;
  }
}
