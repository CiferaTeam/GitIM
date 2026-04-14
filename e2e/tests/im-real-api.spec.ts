import { test, expect } from "@playwright/test";
import { buildRuntime, startEnv, stopEnv, type RuntimeEnv } from "../helpers/runtime-env";

test.describe("IM real API integration", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("/im/me returns handler from me.json", async () => {
    const res = await fetch(`${env.baseUrl}/im/me`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(data.data.handler).toBeTruthy();
    expect(typeof data.data.handler).toBe("string");
    expect(typeof data.data.display_name).toBe("string");
    expect(typeof data.data.guest).toBe("boolean");
  });

  test("/im/users returns user list", async () => {
    const res = await fetch(`${env.baseUrl}/im/users`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(Array.isArray(data.data.users)).toBe(true);
    expect(data.data.users.length).toBeGreaterThan(0);
  });

  test("/im/channels includes general", async () => {
    const res = await fetch(`${env.baseUrl}/im/channels`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(Array.isArray(data.data.channels)).toBe(true);

    const general = data.data.channels.find(
      (c: Record<string, unknown>) => c.name === "general",
    );
    expect(general).toBeDefined();
    expect(general.kind).toBe("channel");
  });

  test("/im/thread returns thread tree", async () => {
    // Send a root message
    const send1 = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "thread root" }),
    });
    const send1Data = await send1.json();
    expect(send1Data.ok).toBe(true);
    const rootLine = send1Data.data.line_number;

    // Reply to it
    const send2 = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        channel: "general",
        body: "thread reply",
        reply_to: rootLine,
      }),
    });
    const send2Data = await send2.json();
    expect(send2Data.ok).toBe(true);

    // Fetch thread
    const threadRes = await fetch(`${env.baseUrl}/im/thread`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", line: rootLine }),
    });
    const threadData = await threadRes.json();
    expect(threadData.ok).toBe(true);
    expect(Array.isArray(threadData.data.entries)).toBe(true);
    expect(threadData.data.entries.length).toBe(2);

    const root = threadData.data.entries[0];
    expect(root.line_number).toBe(rootLine);
    expect(root.body).toBe("thread root");

    const reply = threadData.data.entries[1];
    expect(reply.point_to).toBe(rootLine);
    expect(reply.body).toBe("thread reply");
  });

  test("DM round-trip with dm: prefix format (self-DM)", async () => {
    // Get current handler from /im/me
    const meRes = await fetch(`${env.baseUrl}/im/me`);
    const meData = await meRes.json();
    const handler = meData.data.handler;

    // Use self-DM (dm:handler,handler) to test format without needing a second user
    const dmChannel = `dm:${handler},${handler}`;

    // Send a DM using dm: prefix format
    const sendRes = await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: dmChannel, body: "hello self-dm" }),
    });
    const sendData = await sendRes.json();
    expect(sendData.ok).toBe(true);
    expect(sendData.data.line_number).toBeTruthy();

    // Wait for commit to settle, then read back
    await new Promise((r) => setTimeout(r, 1000));

    const readRes = await fetch(`${env.baseUrl}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: dmChannel }),
    });
    const readData = await readRes.json();
    expect(readData.ok).toBe(true);
    expect(readData.data.entries.length).toBeGreaterThan(0);

    const msg = readData.data.entries.find(
      (e: Record<string, unknown>) => e.body === "hello self-dm",
    );
    expect(msg).toBeDefined();
    expect(msg.author).toBe(handler);

    // Verify DM appears in channels list
    const chRes = await fetch(`${env.baseUrl}/im/channels`);
    const chData = await chRes.json();
    const dmEntry = chData.data.channels.find(
      (c: Record<string, unknown>) => c.kind === "dm",
    );
    expect(dmEntry).toBeDefined();
  });

  test("message format has expected fields", async () => {
    // Send a message with a known body
    await fetch(`${env.baseUrl}/im/send`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general", body: "format check" }),
    });

    // Read and verify field shapes
    const readRes = await fetch(`${env.baseUrl}/im/read`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ channel: "general" }),
    });
    const data = await readRes.json();
    expect(data.ok).toBe(true);

    const msg = data.data.entries.find(
      (e: Record<string, unknown>) => e.body === "format check",
    );
    expect(msg).toBeDefined();
    expect(typeof msg.line_number).toBe("number");
    expect(typeof msg.point_to).toBe("number");
    expect(typeof msg.author).toBe("string");
    expect(typeof msg.timestamp).toBe("string");
    expect(typeof msg.body).toBe("string");
    // Timestamp format: 20260317T120000Z
    expect(msg.timestamp).toMatch(/^\d{8}T\d{6}Z$/);
  });
});
