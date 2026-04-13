import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("agent management API", () => {
  let env: RuntimeEnv;

  test.beforeAll(() => {
    buildRuntime();
  });

  test.beforeEach(async () => {
    env = await startEnv();
  });

  test.afterEach(() => {
    stopEnv(env);
  });

  test("/agents/add + /agents list", async () => {
    const addRes = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "test-agent", display_name: "Test Agent" }),
    });
    expect(addRes.status).toBe(200);
    const addData = await addRes.json();
    expect(addData.ok).toBe(true);
    expect(addData.id).toBe("test-agent");

    const listRes = await fetch(`${env.baseUrl}/agents`);
    expect(listRes.status).toBe(200);
    const listData = await listRes.json();
    expect(listData.ok).toBe(true);
    expect(listData.agents.length).toBe(1);
    expect(listData.agents[0].handler).toBe("test-agent");
    expect(listData.agents[0].status).toBe("idle");
  });

  test("/agents/add duplicate returns error", async () => {
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "dup-agent", display_name: "Dup Agent" }),
    });

    const dupRes = await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "dup-agent", display_name: "Dup Agent" }),
    });
    const dupData = await dupRes.json();
    expect(dupData.ok).toBe(false);
    expect(dupData.error).toMatch(/already exists/);
  });

  test("/agents/start + /agents/stop lifecycle", async () => {
    // Add agent
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        handler: "lifecycle-agent",
        display_name: "Lifecycle Agent",
      }),
    });

    // Start
    const startRes = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-agent" }),
    });
    expect(startRes.status).toBe(200);
    const startData = await startRes.json();
    expect(startData.ok).toBe(true);

    // Verify running
    const list1 = await fetch(`${env.baseUrl}/agents`);
    const data1 = await list1.json();
    const agent1 = data1.agents.find((a: { id: string }) => a.id === "lifecycle-agent");
    expect(agent1).toBeDefined();
    expect(agent1.status).toBe("running");

    // Stop
    const stopRes = await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "lifecycle-agent" }),
    });
    expect(stopRes.status).toBe(200);
    const stopData = await stopRes.json();
    expect(stopData.ok).toBe(true);

    // Verify idle
    const list2 = await fetch(`${env.baseUrl}/agents`);
    const data2 = await list2.json();
    const agent2 = data2.agents.find((a: { id: string }) => a.id === "lifecycle-agent");
    expect(agent2).toBeDefined();
    expect(agent2.status).toBe("idle");
  });

  test("/agents/start on unknown agent returns error", async () => {
    const res = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "ghost-agent" }),
    });
    const data = await res.json();
    expect(data.ok).toBe(false);
    expect(data.error).toMatch(/not found/);
  });

  test("/agents/stop on unknown agent returns error", async () => {
    const res = await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "ghost-agent" }),
    });
    const data = await res.json();
    expect(data.ok).toBe(false);
    expect(data.error).toMatch(/not found/);
  });

  test("/agents/:id returns agent detail", async () => {
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "detail-agent", display_name: "Detail Agent" }),
    });

    const res = await fetch(`${env.baseUrl}/agents/detail-agent`);
    const data = await res.json();
    expect(data.ok).toBe(true);
    expect(data.agent.handler).toBe("detail-agent");
    expect(data.agent.display_name).toBe("Detail Agent");
  });

  test("/agents/remove removes agent", async () => {
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "doomed-agent", display_name: "Doomed Agent" }),
    });

    const removeRes = await fetch(`${env.baseUrl}/agents/remove`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "doomed-agent" }),
    });
    expect((await removeRes.json()).ok).toBe(true);

    const getRes = await fetch(`${env.baseUrl}/agents/doomed-agent`);
    const getData = await getRes.json();
    expect(getData.ok).toBe(false);
  });

  test("/agents/start on already-running agent returns error", async () => {
    await fetch(`${env.baseUrl}/agents/add`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ handler: "running-agent", display_name: "Running Agent" }),
    });

    await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "running-agent" }),
    });

    const res = await fetch(`${env.baseUrl}/agents/start`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "running-agent" }),
    });
    const data = await res.json();
    expect(data.ok).toBe(false);
    expect(data.error).toMatch(/already running/);

    // Clean up
    await fetch(`${env.baseUrl}/agents/stop`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ id: "running-agent" }),
    });
  });
});
