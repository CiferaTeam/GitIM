import { defineConfig, devices } from "@playwright/test";

const externalBaseUrl = process.env.PLAYWRIGHT_BASE_URL;
const localBaseUrl = "http://127.0.0.1:5173";

if (!externalBaseUrl) {
  const noProxy = new Set(
    [process.env.NO_PROXY, process.env.no_proxy]
      .flatMap((value) => value?.split(",") ?? [])
      .map((value) => value.trim())
      .filter(Boolean),
  );
  noProxy.add("127.0.0.1");
  noProxy.add("localhost");
  process.env.NO_PROXY = [...noProxy].join(",");
  process.env.no_proxy = process.env.NO_PROXY;
}

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: "list",
  use: {
    baseURL: externalBaseUrl ?? localBaseUrl,
    trace: "on-first-retry",
  },
  webServer: externalBaseUrl
    ? undefined
    : {
        command: "npm run dev -- --host 127.0.0.1 --port 5173 --strictPort",
        url: localBaseUrl,
        reuseExistingServer: !process.env.CI,
        timeout: 120_000,
      },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
});
