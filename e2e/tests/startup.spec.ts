import { test, expect } from "@playwright/test";
import * as fs from "node:fs";
import * as path from "node:path";
import {
  buildRuntime,
  startServers,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

test.describe("startup flow", () => {
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startServers();
  });

  test.afterAll(() => {
    stopEnv(env);
  });

  test("connect → workspace → git provider → main app", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Connect
    await expect(page.getByTestId("port-input")).toBeVisible();
    await page.getByTestId("port-input").fill(String(env.runtimePort));
    await page.getByTestId("connect-button").click();

    // Workspace
    await expect(page.getByTestId("workspace-input")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("workspace-input").fill(env.workspaceDir);
    await page.getByTestId("workspace-button").click();

    // Git provider
    await expect(page.getByTestId("git-provider-local")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("git-provider-local").click();

    // Main app
    await expect(page.locator("header")).toContainText("GitIM", { timeout: 5000 });

    // Verify artifacts on disk
    expect(fs.existsSync(path.join(env.workspaceDir, ".gitim-runtime", "config.json"))).toBe(true);
    expect(fs.existsSync(path.join(env.workspaceDir, "repo.git", "HEAD"))).toBe(true);
  });
});
