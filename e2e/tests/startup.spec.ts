import { test, expect } from "@playwright/test";
import { execSync, spawn, type ChildProcess } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import * as net from "node:net";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, "../..");
const WEBUI_DIR = path.join(ROOT, "webui-v2");

/** Find a free port on localhost. */
async function freePort(): Promise<number> {
  return new Promise((resolve) => {
    const srv = net.createServer();
    srv.listen(0, "127.0.0.1", () => {
      const addr = srv.address() as net.AddressInfo;
      srv.close(() => resolve(addr.port));
    });
  });
}

/** Wait until an HTTP endpoint responds with expected JSON field. */
async function waitForHealth(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      const data = await res.json();
      if (data.service === "gitim-runtime") return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Runtime did not become healthy at ${url}`);
}

/** Wait until an HTTP endpoint responds (any 200). */
async function waitForHttp(
  url: string,
  timeoutMs = 30_000,
): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(url, { signal: AbortSignal.timeout(2000) });
      if (res.ok) return;
    } catch {
      // not ready yet
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error(`Server did not become available at ${url}`);
}

test.describe("startup flow", () => {
  let runtimeProc: ChildProcess;
  let viteProc: ChildProcess;
  let runtimePort: number;
  let vitePort: number;
  let workspaceDir: string;

  test.beforeAll(async () => {
    // 1. Build runtime binary
    execSync("cargo build -p gitim-runtime", {
      cwd: ROOT,
      stdio: "inherit",
    });

    // 2. Create temp workspace directory
    workspaceDir = fs.mkdtempSync(path.join(os.tmpdir(), "gitim-e2e-"));

    // 3. Start runtime on a free port
    runtimePort = await freePort();
    const runtimeBin = path.join(ROOT, "target/debug/gitim-runtime");
    runtimeProc = spawn(runtimeBin, ["--port", String(runtimePort)], {
      stdio: "pipe",
    });

    // 4. Start webui-v2 dev server on a free port
    vitePort = await freePort();
    viteProc = spawn("npx", ["vite", "--port", String(vitePort), "--strictPort", "--host", "127.0.0.1"], {
      cwd: WEBUI_DIR,
      stdio: "pipe",
      env: { ...process.env, BROWSER: "none" },
    });

    // 5. Wait for both to be ready
    await Promise.all([
      waitForHealth(`http://127.0.0.1:${runtimePort}/health`),
      waitForHttp(`http://127.0.0.1:${vitePort}`),
    ]);
  });

  test.afterAll(() => {
    runtimeProc?.kill();
    viteProc?.kill();

    // Clean up workspace
    if (workspaceDir && fs.existsSync(workspaceDir)) {
      fs.rmSync(workspaceDir, { recursive: true, force: true });
    }
  });

  test("connect to runtime and set workspace", async ({ page }) => {
    // Clear any stored port from previous runs
    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Should see connect form
    await expect(page.getByTestId("port-input")).toBeVisible();

    // Enter runtime port
    await page.getByTestId("port-input").fill(String(runtimePort));
    await page.getByTestId("connect-button").click();

    // Should transition to workspace form
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });

    // Enter workspace path
    await page.getByTestId("workspace-input").fill(workspaceDir);
    await page.getByTestId("workspace-button").click();

    // Should transition to the main app (any element from AppShell)
    // The AppShell has a "GitIM" header text — wait for it
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 5000,
    });

    // Verify marker file was created on disk
    const configPath = path.join(workspaceDir, ".gitim-runtime", "config.json");
    expect(fs.existsSync(configPath)).toBe(true);

    const config = JSON.parse(fs.readFileSync(configPath, "utf-8"));
    expect(config.workspace).toBe(workspaceDir);
    expect(config.created_at).toBeDefined();
  });

  test("shows error for invalid port", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    await expect(page.getByTestId("port-input")).toBeVisible();

    // Enter a port where nothing is running
    const deadPort = await freePort();
    await page.getByTestId("port-input").fill(String(deadPort));
    await page.getByTestId("connect-button").click();

    // Should show error
    await expect(page.getByTestId("connect-error")).toBeVisible({
      timeout: 5000,
    });
  });

  test("creates workspace directory if it does not exist", async ({ page }) => {
    const newDir = path.join(os.tmpdir(), `gitim-e2e-new-${Date.now()}`);

    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Connect first
    await page.getByTestId("port-input").fill(String(runtimePort));
    await page.getByTestId("connect-button").click();
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });

    // Enter a path that doesn't exist yet
    await page.getByTestId("workspace-input").fill(newDir);
    await page.getByTestId("workspace-button").click();

    // Should succeed — directory gets created
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 5000,
    });

    // Verify directory and marker were created
    expect(fs.existsSync(path.join(newDir, ".gitim-runtime", "config.json"))).toBe(true);

    // Cleanup
    fs.rmSync(newDir, { recursive: true, force: true });
  });

  test("prompts confirmation for non-empty workspace directory", async ({ page }) => {
    // Create a non-empty directory
    const dirtyDir = fs.mkdtempSync(path.join(os.tmpdir(), "gitim-e2e-dirty-"));
    fs.writeFileSync(path.join(dirtyDir, "existing-file.txt"), "hello");

    await page.goto(`http://127.0.0.1:${vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    // Connect first
    await page.getByTestId("port-input").fill(String(runtimePort));
    await page.getByTestId("connect-button").click();
    await expect(page.getByTestId("workspace-input")).toBeVisible({
      timeout: 5000,
    });

    // Enter non-empty directory
    await page.getByTestId("workspace-input").fill(dirtyDir);
    await page.getByTestId("workspace-button").click();

    // Should show warning and confirm button
    await expect(page.getByTestId("workspace-error")).toBeVisible({
      timeout: 5000,
    });
    await expect(page.getByTestId("workspace-confirm-button")).toBeVisible();

    // Confirm
    await page.getByTestId("workspace-confirm-button").click();

    // Should proceed to main app
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 5000,
    });

    // Cleanup
    fs.rmSync(dirtyDir, { recursive: true, force: true });
  });
});
