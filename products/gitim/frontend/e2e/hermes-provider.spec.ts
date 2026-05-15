import { test, expect } from "@playwright/test";

test.describe("Hermes Provider", () => {
  test("Add flow runs server-side preflight", async ({ page }) => {
    // Set runtime port to 3000 (daemon debug HTTP)
    await page.goto("http://localhost:5174");
    await page.evaluate(() => {
      localStorage.setItem("gitim-runtime-port", "3000");
    });

    // Reload to apply port
    await page.reload();

    // Wait for the app to load (connection check)
    await page.waitForTimeout(2000);

    // Wait for Add Agent button
    await page.waitForSelector("text=Add Agent", { timeout: 10000 });

    // Click Add Agent button
    await page.click("text=Add Agent");

    // Wait for dialog
    await page.waitForSelector("text=Provider");

    // Select Hermes provider
    await page.selectOption("#agent-provider", "hermes");

    // Fill agent name and submit — preflight now runs inline as part of
    // `POST /agents/add`. The dialog either closes on success or renders a
    // sticky preflight-failure block (`Provider not installed` / `Timed out`
    // / `Other error`) sourced from the server's `preflight_detail`.
    await page.fill("#agent-name", "Test Hermes Agent");
    await page.click("text=Add agent");

    const outcome = await page.locator("text=Test Hermes Agent")
      .or(page.locator("text=Provider not installed"))
      .or(page.locator("text=Timed out"))
      .or(page.locator("text=Other error"))
      .first();
    await expect(outcome).toBeVisible({ timeout: 20000 });
  });
});
