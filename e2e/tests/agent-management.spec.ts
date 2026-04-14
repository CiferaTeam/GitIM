import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("agent management API", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("add → list → start → stop → remove lifecycle", async () => {
    // Add
    const addRes = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "lifecycle-bot", display_name: "Lifecycle Bot", provider: "mock" }),
    });
    const addData = await addRes.json();
    expect(addData.ok).toBe(true);
    expect(addData.id).toBe("lifecycle-bot");

    // List
    const listRes = await fetch(`${env.baseUrl}/agents`);
    const listData = await listRes.json();
    expect(listData.ok).toBe(true);
    expect(listData.agents.some((a: { id: string }) => a.id === "lifecycle-bot")).toBe(true);

    // Start
    const startRes = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-bot" }),
    });
    expect((await startRes.json()).ok).toBe(true);

    // Verify running
    const list2 = await (await fetch(`${env.baseUrl}/agents`)).json();
    expect(list2.agents.find((a: { id: string }) => a.id === "lifecycle-bot").status).toBe("running");

    // Stop
    const stopRes = await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-bot" }),
    });
    expect((await stopRes.json()).ok).toBe(true);

    // Verify idle
    const list3 = await (await fetch(`${env.baseUrl}/agents`)).json();
    expect(list3.agents.find((a: { id: string }) => a.id === "lifecycle-bot").status).toBe("idle");

    // Remove
    const removeRes = await fetch(`${env.baseUrl}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-bot" }),
    });
    expect((await removeRes.json()).ok).toBe(true);

    // Verify gone
    const list4 = await (await fetch(`${env.baseUrl}/agents`)).json();
    expect(list4.agents.some((a: { id: string }) => a.id === "lifecycle-bot")).toBe(false);
  });
});
