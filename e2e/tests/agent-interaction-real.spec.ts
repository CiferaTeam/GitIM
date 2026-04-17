import { test, expect } from "@playwright/test";
import { buildRuntime, startEnv, stopEnv, type RuntimeEnv } from "../helpers/runtime-env";

// This spec spins up real Claude + Codex CLIs and consumes real API tokens
// (a few cents per run). Gate behind E2E_REAL_PROVIDERS — default: skipped.
// Run locally after `claude login` / `codex login` with:
//   E2E_REAL_PROVIDERS=1 npx playwright test agent-interaction-real
const REAL = !!process.env.E2E_REAL_PROVIDERS;
const describe = REAL ? test.describe : test.describe.skip;

describe("real Claude + Codex round-trip", () => {
  // Real LLM round-trip is slow: daemon push → agent pull → poll → LLM call →
  // reply → push → human pull. Two agents in parallel, each taking ~20-60s.
  test.setTimeout(360_000);

  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(async () => {
    // Best-effort stop of both agents before tearing down the env.
    for (const id of ["claude-bot", "codex-bot"]) {
      try {
        await fetch(`${env.baseUrl}/agents/stop`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ id }),
        });
      } catch {
        // env may already be stopping
      }
    }
    stopEnv(env);
  });

  test("both agents receive mentions and reply with the expected text", async () => {
    // 1. Add Claude agent (haiku keeps cost low)
    const addClaude = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler: "claude-bot",
        display_name: "Claude Bot",
        provider: "claude",
        model: "claude-haiku-4-5",
      }),
    });
    expect((await addClaude.json()).ok).toBe(true);

    // 2. Add Codex agent (gpt-5.4-mini keeps cost low)
    const addCodex = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler: "codex-bot",
        display_name: "Codex Bot",
        provider: "codex",
        model: "gpt-5.4-mini",
      }),
    });
    expect((await addCodex.json()).ok).toBe(true);

    // 3. Start both agent loops. /agents/add auto-starts the loop, so these
    //    calls are idempotent — they exist to match the spec and as a safety net.
    for (const id of ["claude-bot", "codex-bot"]) {
      const startRes = await fetch(`${env.baseUrl}/agents/start`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id }),
      });
      expect((await startRes.json()).ok).toBe(true);
    }

    // 4. Initialize poll cursor so we only observe messages created after setup.
    const poll0 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const cursor0 = (await poll0.json()).data?.commit_id;

    // 5. Send one prompt per agent on the default "general" channel. Explicit
    //    @mention makes the trigger unambiguous for both providers.
    const prompts = [
      { channel: "general", body: "@claude-bot reply with exactly: CLAUDE_HELLO" },
      { channel: "general", body: "@codex-bot reply with exactly: CODEX_HELLO" },
    ];
    for (const p of prompts) {
      const sendRes = await fetch(`${env.baseUrl}/im/send`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(p),
      });
      expect((await sendRes.json()).ok).toBe(true);
    }

    // 6. Poll until both expected tokens appear, or 300s deadline.
    //    Codex's first turn in a fresh agent repo spends ~2-3 minutes exploring
    //    (30+ Bash calls) before replying, so the cold-start budget has to
    //    absorb that worst case.
    let sawClaude = false;
    let sawCodex = false;
    const deadline = Date.now() + 300_000;

    while (Date.now() < deadline && !(sawClaude && sawCodex)) {
      const pollRes = await fetch(`${env.baseUrl}/im/poll`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ since: cursor0 }),
      });
      const pollData = await pollRes.json();

      if (pollData.ok && pollData.data?.changes) {
        for (const change of pollData.data.changes) {
          for (const entry of change.entries ?? []) {
            // Only count messages authored by the agents — the human prompt
            // itself quotes "CLAUDE_HELLO"/"CODEX_HELLO", so a plain substring
            // search would false-match on the outbound human message.
            if (entry.type !== "message") continue;
            const body: string = entry.body ?? "";
            if (entry.author === "claude-bot" && body.includes("CLAUDE_HELLO")) {
              sawClaude = true;
            }
            if (entry.author === "codex-bot" && body.includes("CODEX_HELLO")) {
              sawCodex = true;
            }
          }
        }
      }
      if (sawClaude && sawCodex) break;
      await new Promise((r) => setTimeout(r, 2000));
    }

    expect(sawClaude, "Claude bot did not reply with CLAUDE_HELLO within 300s").toBe(true);
    expect(sawCodex, "Codex bot did not reply with CODEX_HELLO within 300s").toBe(true);
  });
});
