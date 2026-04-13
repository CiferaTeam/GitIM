import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startServers,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("UI agent CRUD (real backend)", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startServers();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("add agent via UI triggers real backend provisioning", async ({
    page,
  }) => {
    // 1. Go through full UI startup flow
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Connect
    await page.getByTestId("port-input").fill(String(env.runtimePort));
    await page.getByTestId("connect-button").click();

    // Workspace
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });
    await page.getByTestId("workspace-input").fill(env.workspaceDir);
    await page.getByTestId("workspace-button").click();

    // Git provider
    await expect(page.getByTestId("git-provider-local")).toBeVisible({
      timeout: 5000,
    });
    await page.getByTestId("git-provider-local").click();

    // Wait for main app
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 10000,
    });

    // 2. Add agent via UI
    await page.getByRole("button", { name: "Add Agent" }).click();
    await page.getByLabel("Name").fill("E2E Bot");
    await page.getByLabel("System Prompt").fill("test prompt");
    await page.getByRole("button", { name: "Add", exact: true }).click();

    // Wait for agent to appear in UI
    await expect(page.getByText("E2E Bot")).toBeVisible({ timeout: 10000 });

    // 3. Verify backend actually provisioned the agent
    const listRes = await fetch(`${env.baseUrl}/agents`);
    const listData = await listRes.json();
    expect(listData.ok).toBe(true);

    const agent = listData.agents.find(
      (a: Record<string, unknown>) => a.handler === "e2e-bot",
    );
    expect(agent).toBeDefined();
    expect(agent.display_name).toBe("E2E Bot");
    expect(agent.status).toBe("idle");
  });
});
