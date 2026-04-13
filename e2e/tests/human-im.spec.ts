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

  test("/im/me returns human identity", async () => {
    const res = await fetch(`${env.baseUrl}/im/me`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(data.data).toBeDefined();
  });

  test("/im/channels returns channel list", async () => {
    const res = await fetch(`${env.baseUrl}/im/channels`);
    const data = await res.json();
    expect(data.ok).toBe(true);
  });

  test("/im/send + /im/read round-trip", async () => {
    // Send a message to "general" channel
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
    const readData = await readRes.json();
    expect(readData.ok).toBe(true);
  });

  test("/im/poll returns changes since cursor", async () => {
    // First poll — record the current HEAD cursor
    const poll1 = await fetch(`${env.baseUrl}/im/poll`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({}),
    });
    const data1 = await poll1.json();
    expect(data1.ok).toBe(true);
    const cursor = data1.data?.commit_id;
    expect(cursor).toBeDefined();

    // Send a message to the already-existing "general" channel
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "poll message" }),
    });
    expect((await sendRes.json()).ok).toBe(true);

    // Poll again with the saved cursor — should see the new commit
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
