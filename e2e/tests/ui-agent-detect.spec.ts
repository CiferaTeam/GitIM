import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

// Real preflight hits the provider CLIs; add-agent writes into the workspace
// via a live daemon. Gated behind E2E_REAL_PROVIDERS — default: skipped.
// Run locally after `claude login` / `codex login` with:
//   E2E_REAL_PROVIDERS=1 npx playwright test ui-agent-detect
const REAL = !!process.env.E2E_REAL_PROVIDERS;
const describe = REAL ? test.describe : test.describe.skip;

describe("UI Detect button + add claude and codex agents", () => {
  // UI startup + two live preflights (claude ~2s, codex ~10s) + two agent
  // creations. Budget generously so flaky-network retries don't blow up.
  test.setTimeout(360_000);

  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(async () => {
    // Best-effort stop before teardown — agent_loop is running as a child
    // process and deserves a clean SIGTERM before the workspace disappears.
    for (const id of ["claude-bot", "codex-bot"]) {
      try {
        await fetch(`${env.baseUrl}/agents/stop`, {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ id }),
        });
      } catch {
        // env may already be stopping
      }
    }
    stopEnv(env);
  });

  test("Detect gates Add; claude + codex both land in the list", async ({
    page,
  }) => {
    // startEnv already drove /workspace + /git/init. Plant the port in
    // localStorage *before* the app mounts so SetupGate's auto-connect sees
    // `initialized: true` from /health and jumps straight to "ready",
    // skipping the Workspace + GitProvider forms.
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate((port) => {
      localStorage.clear();
      localStorage.setItem("gitim-runtime-port", String(port));
    }, env.runtimePort);
    await page.reload();

    // Land on /management (default route). Header confirms main app is up.
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 10_000,
    });

    // ── claude-bot ────────────────────────────────────────────────────────
    await page.getByRole("button", { name: /Add Agent/i }).click();

    // Provider first — model <select> is disabled until a provider is picked.
    const providerSelect = page.locator("#agent-provider");
    await expect(providerSelect).toBeVisible({ timeout: 5000 });
    await providerSelect.selectOption("claude");

    const modelSelect = page.locator("#agent-model");
    await expect(modelSelect).toBeEnabled();
    await modelSelect.selectOption("claude-haiku-4-5");

    // Name after model so the disabled Add button toggle hinges on detect.
    await page.getByLabel("Name").fill("claude-bot");

    // Before detect: Add must be disabled.
    const addButton = page.getByRole("button", { name: /^Add$/i });
    await expect(addButton).toBeDisabled();

    await page.getByRole("button", { name: /^Detect$/i }).click();

    // Real preflight against the claude CLI — typically ~2s, but be generous.
    // Scope to the dialog so the assertion doesn't race with leftover DOM.
    const dialog = page.getByRole("dialog");
    await expect(dialog.locator("text=OK —")).toBeVisible({ timeout: 90_000 });

    await expect(addButton).toBeEnabled();
    await addButton.click();

    // Dialog closes → Add Agent button reappears in the list header.
    await expect(page.getByRole("button", { name: /Add Agent/i })).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByText("claude-bot")).toBeVisible({ timeout: 10_000 });

    // ── codex-bot ─────────────────────────────────────────────────────────
    await page.getByRole("button", { name: /Add Agent/i }).click();

    const providerSelect2 = page.locator("#agent-provider");
    await expect(providerSelect2).toBeVisible({ timeout: 5000 });
    await providerSelect2.selectOption("codex");

    const modelSelect2 = page.locator("#agent-model");
    await expect(modelSelect2).toBeEnabled();
    await modelSelect2.selectOption("gpt-5.4");

    await page.getByLabel("Name").fill("codex-bot");

    const addButton2 = page.getByRole("button", { name: /^Add$/i });
    await expect(addButton2).toBeDisabled();

    await page.getByRole("button", { name: /^Detect$/i }).click();

    // Codex preflight is slower (~10s cold) — same 90s ceiling is fine.
    const dialog2 = page.getByRole("dialog");
    await expect(dialog2.locator("text=OK —")).toBeVisible({ timeout: 90_000 });

    await expect(addButton2).toBeEnabled();
    await addButton2.click();

    await expect(page.getByRole("button", { name: /Add Agent/i })).toBeVisible({
      timeout: 10_000,
    });
    await expect(page.getByText("codex-bot")).toBeVisible({ timeout: 10_000 });

    // Both agents persisted — sanity-check via backend, independent of UI state.
    const listRes = await fetch(`${env.baseUrl}/agents`);
    const listData = await listRes.json();
    expect(listData.ok).toBe(true);
    const handlers = listData.agents.map(
      (a: Record<string, unknown>) => a.handler,
    );
    expect(handlers).toContain("claude-bot");
    expect(handlers).toContain("codex-bot");
  });
});
