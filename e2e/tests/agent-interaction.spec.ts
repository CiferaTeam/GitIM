import { test, expect } from "@playwright/test";
import { buildRuntime, startEnv, stopEnv, type RuntimeEnv } from "../helpers/runtime-env";

test.describe("human-agent interaction", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("human sends message, agent replies via MockProvider", async () => {
    // 1. Add and start agent
    const addRes = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "mock-bot", display_name: "Mock Bot", provider: "mock" }),
    });
    expect((await addRes.json()).ok).toBe(true);

    const startRes = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "mock-bot" }),
    });
    expect((await startRes.json()).ok).toBe(true);

    // 2. Initialize poll cursor (so we only see NEW changes after this point)
    const poll0 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const cursor0 = (await poll0.json()).data?.commit_id;

    // 3. Human sends a message on "general" channel
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "hello agent" }),
    });
    expect((await sendRes.json()).ok).toBe(true);

    // 4. Wait for agent to process and reply
    // Flow: human daemon pushes → agent daemon pulls → agent_loop polls →
    //       MockProvider sends reply → agent daemon pushes → human daemon pulls
    // Agent polls every 2s, git sync has intervals too. Allow up to 30s.
    let agentReplied = false;
    const deadline = Date.now() + 30_000;

    while (Date.now() < deadline) {
      const pollRes = await fetch(`${env.baseUrl}/im/poll`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ since: cursor0 }),
      });
      const pollData = await pollRes.json();

      if (pollData.ok && pollData.data?.changes) {
        for (const change of pollData.data.changes) {
          for (const entry of change.entries ?? []) {
            // Look for mock-response in any field
            const entryStr = JSON.stringify(entry);
            if (entryStr.includes("mock-response")) {
              agentReplied = true;
              break;
            }
          }
          if (agentReplied) break;
        }
      }
      if (agentReplied) break;
      await new Promise((r) => setTimeout(r, 2000));
    }

    expect(agentReplied).toBe(true);

    // 5. Cleanup: stop agent
    await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "mock-bot" }),
    });
  });
});
