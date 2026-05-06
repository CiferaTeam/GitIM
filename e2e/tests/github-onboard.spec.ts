import { test, expect } from "@playwright/test";
import { execSync } from "node:child_process";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { startStubGithubApi, type StubServer } from "../helpers/github-stub";
import {
  buildRuntime,
  startServers,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

// macOS unix-socket path limit (104) forces short workspace paths.
const TMP_BASE = process.platform === "darwin" ? "/tmp" : os.tmpdir();

/** Seed a bare repo with one commit so `git clone` produces a working tree. */
function seedBareRepo(bareDir: string): string {
  const barePath = path.join(bareDir, "fake.git");
  execSync(`git init --bare --initial-branch=main "${barePath}"`, {
    stdio: "pipe",
  });
  const seedDir = path.join(bareDir, "seed");
  execSync(`git clone "${barePath}" "${seedDir}"`, { stdio: "pipe" });
  execSync(
    [
      "git -c init.defaultBranch=main",
      `-C "${seedDir}"`,
      "config user.email test@example.com",
    ].join(" "),
    { stdio: "pipe" },
  );
  execSync(`git -C "${seedDir}" config user.name Test`, { stdio: "pipe" });
  fs.writeFileSync(path.join(seedDir, ".keep"), "");
  execSync(`git -C "${seedDir}" add .keep`, { stdio: "pipe" });
  execSync(`git -C "${seedDir}" commit -m init`, { stdio: "pipe" });
  execSync(`git -C "${seedDir}" push origin main`, { stdio: "pipe" });
  fs.rmSync(seedDir, { recursive: true, force: true });
  return `file://${barePath}`;
}

test.describe("github-mode onboard", () => {
  test.describe.configure({ mode: "serial" });

  let stub: StubServer;
  let bareDir: string;
  let fakeRemoteUrl: string;
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();

    stub = await startStubGithubApi();

    bareDir = fs.mkdtempSync(path.join(TMP_BASE, "gitim-fake-gh-"));
    fakeRemoteUrl = seedBareRepo(bareDir);

    env = await startServers({
      runtimeEnv: {
        GITIM_TEST_GITHUB_API_BASE: stub.baseUrl,
        GITIM_TEST_CLONE_URL_OVERRIDE: fakeRemoteUrl,
      },
    });
  });

  test.afterAll(async () => {
    stopEnv(env);
    await stub.close();
    if (bareDir && fs.existsSync(bareDir)) {
      fs.rmSync(bareDir, { recursive: true, force: true });
    }
  });

  test("happy path: connect → workspace → github → ready", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    await expect(page.getByTestId("port-input")).toBeVisible();
    await page.getByTestId("port-input").fill(String(env.runtimePort));
    await page.getByTestId("connect-button").click();

    await expect(page.getByTestId("workspace-input")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("workspace-input").fill(env.workspaceDir);
    await page.getByTestId("workspace-button").click();

    await expect(page.getByTestId("git-provider-github")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("git-provider-github").click();

    await expect(page.getByTestId("github-remote-url")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("github-remote-url").fill("https://github.com/fake/fake");
    await page.getByTestId("github-token").fill("ghp_fake_token_for_testing");
    await page.getByTestId("github-ack").check();

    // Capture the /git/init response so a failure message is visible in
    // the test log without having to re-enable debug tracing.
    const gitInitPromise = page.waitForResponse(
      (res) => res.url().endsWith("/git/init") && res.request().method() === "POST",
      { timeout: 30_000 },
    );
    await page.getByTestId("github-connect-button").click();
    const gitInitRes = await gitInitPromise;
    const gitInitJson = await gitInitRes.json();
    if (!gitInitJson.ok) {
      throw new Error(`/git/init failed: ${JSON.stringify(gitInitJson)}`);
    }

    // Reaches the main app — SetupGate swaps in children on status=ready.
    await expect(page.locator("header")).toContainText("GitIM", { timeout: 30_000 });

    // Artifacts on disk
    expect(fs.existsSync(path.join(env.workspaceDir, ".gitim-runtime", "config.json"))).toBe(true);
    expect(fs.existsSync(path.join(env.workspaceDir, ".gitim-runtime", "human"))).toBe(true);
    expect(fs.existsSync(path.join(env.workspaceDir, ".gitim-runtime", "human", ".git"))).toBe(true);

    // Workspace config carries github provider + PAT — the second write
    // (from /git/init) uses the full schema and shadows the marker config.
    const workspaceConfigPath = path.join(env.workspaceDir, ".gitim-runtime", "config.json");
    const workspaceConfig = JSON.parse(fs.readFileSync(workspaceConfigPath, "utf8"));
    expect(workspaceConfig.git.provider).toBe("github");
    expect(workspaceConfig.git.remote_url).toBe("https://github.com/fake/fake");
    expect(workspaceConfig.git.token).toBe("ghp_fake_token_for_testing");

    // Stub was called for both verify + repo access.
    expect(stub.hits.some((h) => h.includes("GET /user"))).toBe(true);
    expect(stub.hits.some((h) => h.includes("GET /repos/fake/fake"))).toBe(true);
  });
});

test.describe("github-mode invalid token", () => {
  test.describe.configure({ mode: "serial" });

  let stub: StubServer;
  let bareDir: string;
  let fakeRemoteUrl: string;
  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();

    stub = await startStubGithubApi({ user: { status: 401 } });

    bareDir = fs.mkdtempSync(path.join(TMP_BASE, "gitim-fake-gh-"));
    fakeRemoteUrl = seedBareRepo(bareDir);

    env = await startServers({
      runtimeEnv: {
        GITIM_TEST_GITHUB_API_BASE: stub.baseUrl,
        GITIM_TEST_CLONE_URL_OVERRIDE: fakeRemoteUrl,
      },
    });
  });

  test.afterAll(async () => {
    stopEnv(env);
    await stub.close();
    if (bareDir && fs.existsSync(bareDir)) {
      fs.rmSync(bareDir, { recursive: true, force: true });
    }
  });

  test("rejected PAT surfaces invalid_token error", async ({ page }) => {
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate(() => localStorage.clear());
    await page.reload();

    await page.getByTestId("port-input").fill(String(env.runtimePort));
    await page.getByTestId("connect-button").click();

    await expect(page.getByTestId("workspace-input")).toBeVisible({ timeout: 5000 });
    await page.getByTestId("workspace-input").fill(env.workspaceDir);
    await page.getByTestId("workspace-button").click();

    await page.getByTestId("git-provider-github").click();
    await page.getByTestId("github-remote-url").fill("https://github.com/fake/fake");
    await page.getByTestId("github-token").fill("ghp_bogus");
    await page.getByTestId("github-ack").check();
    await page.getByTestId("github-connect-button").click();

    await expect(page.getByTestId("github-setup-error")).toContainText(
      "Token was rejected",
      { timeout: 15_000 },
    );

    // Failed verify must not create a human clone on disk.
    expect(fs.existsSync(path.join(env.workspaceDir, ".gitim-runtime", "human"))).toBe(false);
  });
});
