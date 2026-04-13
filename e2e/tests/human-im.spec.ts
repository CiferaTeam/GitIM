import { test, expect } from "@playwright/test";
import { buildRuntime, startEnv, stopEnv, type RuntimeEnv } from "../helpers/runtime-env";

test.describe("human IM API", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("send → read → poll round-trip", async () => {
    // Poll to get initial cursor
    const poll1 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const data1 = await poll1.json();
    expect(data1.ok).toBe(true);
    const cursor = data1.data?.commit_id;

    // Send a message
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "hello from e2e" }),
    });
    expect((await sendRes.json()).ok).toBe(true);

    // Read it back
    const readRes = await fetch(`${env.baseUrl}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general" }),
    });
    expect((await readRes.json()).ok).toBe(true);

    // Poll should show changes
    const poll2 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ since: cursor }),
    });
    const data2 = await poll2.json();
    expect(data2.ok).toBe(true);
    expect(data2.data?.changes?.length).toBeGreaterThan(0);
  });
});
