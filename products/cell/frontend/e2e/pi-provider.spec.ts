import { test, expect } from "@playwright/test";

test.describe("Pi Provider", () => {
  test("Detect + Add flow", async ({ page }) => {
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

    // Click Detect
    await page.click("text=Detect");

    // Wait for detect result (success or failure)
    await page.waitForTimeout(5000);

    // Check if detect completed (either success or error)
    const detectResult = await page.locator("text=OK").or(page.locator("text=not found")).or(page.locator("text=Timed out")).first();
    await expect(detectResult).toBeVisible({ timeout: 15000 });

    // If detect succeeded, fill in name and submit
    const okResult = await page.locator("text=OK").first();
    if (await okResult.isVisible().catch(() => false)) {
      // Fill agent name
      await page.fill("#agent-name", "Test Pi Agent");

      // Submit
      await page.click("text=Add");

      // Wait for dialog to close
      await page.waitForSelector("text=Add Agent", { timeout: 10000 });
    }
  });
});
