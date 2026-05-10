import { test, expect } from "@playwright/test";
import {
  buildRuntime,
  startEnv,
  stopEnv,
  type RuntimeEnv,
} from "../helpers/runtime-env";

// Spins up a real hermes + minimax-cn LLM round-trip, which costs real
// API tokens (~cents) and requires a configured hermes home with a minimax
// API key. Gate behind E2E_REAL_PROVIDERS — default: skipped.
// Run locally after `hermes setup` with:
//   E2E_REAL_PROVIDERS=1 pnpm -C e2e exec playwright test ui-hermes-llm
const REAL = !!process.env.E2E_REAL_PROVIDERS;
const describe = REAL ? test.describe : test.describe.skip;

describe("UI hermes LLM provider/model selection", () => {
  // Covers: /providers fetch (~2s) + live /models fetch from minimax API
  // (~5-10s) + real hermes preflight LLM call (~10-20s) + add-agent write.
  // 180s covers worst-case network latency on all three live calls.
  test.setTimeout(180_000);

  let env: RuntimeEnv;

  test.beforeAll(async () => {
    buildRuntime();
    env = await startEnv();
  });

  test.afterAll(async () => {
    // Best-effort stop before teardown — agent_loop is a child process.
    try {
      await fetch(`${env.baseUrl}/agents/stop`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ id: "hermes-bot" }),
      });
    } catch {
      // env may already be stopping
    }
    stopEnv(env);
  });

  test("user adds hermes agent with selected LLM via UI", async ({ page }) => {
    // startEnv already completed /workspace + /git/init. Seed the port into
    // localStorage before the React app mounts so SetupGate's auto-connect
    // sees `initialized: true` from /health and lands directly in "ready".
    await page.goto(`http://127.0.0.1:${env.vitePort}`);
    await page.evaluate((port) => {
      localStorage.clear();
      localStorage.setItem("gitim-runtime-port", String(port));
    }, env.runtimePort);
    await page.reload();

    // Confirm the main app rendered — header should contain "GitIM".
    await expect(page.locator("header")).toContainText("GitIM", {
      timeout: 10_000,
    });

    // ── Open AddAgentDialog ────────────────────────────────────────────────
    await page.getByRole("button", { name: /Add Agent/i }).click();

    const dialog = page.getByRole("dialog");
    await expect(dialog).toBeVisible({ timeout: 5_000 });

    // ── Pick Hermes provider ───────────────────────────────────────────────
    const providerSelect = page.locator("#agent-provider");
    await expect(providerSelect).toBeVisible({ timeout: 5_000 });
    await providerSelect.selectOption("hermes");

    // ── Fill agent name ────────────────────────────────────────────────────
    // Name after provider so the Add button's disabled check (hermesLlmIncomplete)
    // can gate on both name and LLM selection independently.
    await page.locator("#agent-name").fill("hermes-bot");

    // ── Wait for "Hermes LLM" section to appear and providers to load ─────
    // The section is rendered only when provider === "hermes". The loading
    // spinner ("Loading providers…") disappears once the /providers response
    // arrives (~2s typical). Timeout 10s to tolerate slow runtime starts.
    await expect(dialog.locator("text=Loading providers…")).toBeVisible({
      timeout: 5_000,
    });
    await expect(dialog.locator("text=Loading providers…")).toBeHidden({
      timeout: 10_000,
    });

    // ── Select minimax-cn as the LLM provider ─────────────────────────────
    // Provider id "minimax-cn" is a builtin in hermes. If the test environment
    // doesn't have minimax-cn, fall back to the first available option by
    // reading the select's options rather than hardcoding the value.
    const llmProviderSelect = page.locator("#hermes-llm-provider");
    await expect(llmProviderSelect).toBeEnabled({ timeout: 5_000 });

    // Attempt to select minimax-cn; if not present, select the first non-empty
    // option so the test still exercises the full path.
    const optionValues = await llmProviderSelect.evaluate((sel: HTMLSelectElement) =>
      Array.from(sel.options)
        .map((o) => o.value)
        .filter((v) => v !== ""),
    );
    const targetProvider = optionValues.includes("minimax-cn")
      ? "minimax-cn"
      : optionValues[0];
    if (!targetProvider) {
      throw new Error(
        "No LLM providers available in hermes home. Run `hermes setup` and add an API key.",
      );
    }
    await llmProviderSelect.selectOption(targetProvider);

    // ── Wait for model list to load ────────────────────────────────────────
    // Live fetch from the provider's /models endpoint; minimax-cn is fast but
    // we allow 15s for slow networks.
    await expect(dialog.locator("text=Loading models…")).toBeVisible({
      timeout: 5_000,
    });
    await expect(dialog.locator("text=Loading models…")).toBeHidden({
      timeout: 15_000,
    });

    // ── Select a model ─────────────────────────────────────────────────────
    // Prefer MiniMax-M2.7-highspeed; fall back to first available model or
    // use the Custom… input if the live fetch returned an error.
    const llmModelSelect = page.locator("#hermes-llm-model");
    const isSelectVisible = await llmModelSelect.isVisible();

    if (isSelectVisible) {
      const modelValues = await llmModelSelect.evaluate((sel: HTMLSelectElement) =>
        Array.from(sel.options)
          .map((o) => o.value)
          .filter((v) => v !== "" && v !== "__custom__"),
      );
      const targetModel = modelValues.includes("MiniMax-M2.7-highspeed")
        ? "MiniMax-M2.7-highspeed"
        : modelValues[0];
      if (!targetModel) {
        // Fall through to Custom… input if no builtin models returned.
        await llmModelSelect.selectOption("__custom__");
        await page.locator("#hermes-llm-model").fill("MiniMax-M2.7-highspeed");
      } else {
        await llmModelSelect.selectOption(targetModel);
      }
    } else {
      // Model list fetch returned an error — the dialog renders the custom
      // text input directly. Fill it with a known-good model id.
      const customInput = page.locator("#hermes-llm-model");
      await expect(customInput).toBeVisible({ timeout: 5_000 });
      await customInput.fill("MiniMax-M2.7-highspeed");
    }

    // ── Detect (real preflight LLM call) ──────────────────────────────────
    // The Detect button is enabled once provider + llmProvider + effectiveModel
    // are all non-empty. Click and wait for green "OK —" feedback.
    const detectButton = page.getByRole("button", { name: /^Detect$/i });
    await expect(detectButton).toBeEnabled({ timeout: 5_000 });
    await detectButton.click();

    // Real hermes preflight makes an LLM call; budget 30s.
    await expect(dialog.locator("text=OK —")).toBeVisible({ timeout: 30_000 });

    // ── Add button should now be enabled ──────────────────────────────────
    const addButton = page.getByRole("button", { name: /^Add$/i });
    await expect(addButton).toBeEnabled({ timeout: 5_000 });

    // ── Submit ─────────────────────────────────────────────────────────────
    await addButton.click();

    // Dialog closes → "Add Agent" button reappears in the page header.
    await expect(page.getByRole("button", { name: /Add Agent/i })).toBeVisible({
      timeout: 10_000,
    });
    // Agent card should appear in the list.
    await expect(page.getByText("hermes-bot")).toBeVisible({ timeout: 10_000 });

    // ── Backend sanity-check ───────────────────────────────────────────────
    // Verify the agent was persisted independently of UI state.
    const listRes = await fetch(`${env.baseUrl}/agents`);
    const listData = (await listRes.json()) as {
      ok: boolean;
      agents: { handler: string }[];
    };
    expect(listData.ok).toBe(true);
    const handlers = listData.agents.map((a) => a.handler);
    expect(handlers).toContain("hermes-bot");
  });
});
