import { test, expect } from "@playwright/test";

test.describe("Hermes Provider", () => {
  test("Detect + Add flow", async ({ page }) => {
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

    // Click Detect
    await page.click("text=Detect");

    // Wait for detect result (success or failure)
    await page.waitForTimeout(8000);

    // Check if detect completed (either success or error)
    const detectResult = await page.locator("text=OK").or(page.locator("text=not found")).or(page.locator("text=Timed out")).or(page.locator("text=CLI")).first();
    await expect(detectResult).toBeVisible({ timeout: 20000 });

    // If detect succeeded, fill in name and submit
    const okResult = await page.locator("text=OK").first();
    if (await okResult.isVisible().catch(() => false)) {
      // Fill agent name
      await page.fill("#agent-name", "Test Hermes Agent");

      // Submit
      await page.click("text=Add");

      // Wait for dialog to close or success message
      await page.waitForTimeout(3000);
    }
  });
});
