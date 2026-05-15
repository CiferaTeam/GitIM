import { test, expect } from "@playwright/test";

test.describe("Pi Provider", () => {
  test("Add flow runs server-side preflight", async ({ page }) => {
    // Navigate to the app
    await page.goto("http://localhost:5173");

    // Wait for the app to load
    await page.waitForSelector("text=Add Agent", { timeout: 10000 });

    // Click Add Agent button
    await page.click("text=Add Agent");

    // Wait for dialog
    await page.waitForSelector("text=Provider");

    // Select Pi provider
    await page.selectOption("#agent-provider", "pi");

    // Fill agent name and submit — preflight now runs inline as part of
    // `POST /agents/add`. The dialog either closes on success or renders a
    // sticky preflight-failure block (`Provider not installed` / `Timed out`
    // / `Other error`) sourced from the server's `preflight_detail`.
    await page.fill("#agent-name", "Test Pi Agent");
    await page.click("text=Add agent");

    const outcome = await page.locator("text=Test Pi Agent")
      .or(page.locator("text=Provider not installed"))
      .or(page.locator("text=Timed out"))
      .or(page.locator("text=Other error"))
      .first();
    await expect(outcome).toBeVisible({ timeout: 15000 });
  });
});
