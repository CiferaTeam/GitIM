import { expect, test, type Page } from "@playwright/test";

const runtimePort = 49322;
const slug = "mobile";

async function stubRuntime(page: Page) {
  await page.addInitScript(({ port, activeSlug }) => {
    localStorage.clear();
    localStorage.setItem("gitim-runtime-port", String(port));
    localStorage.setItem("gitim-active-workspace", activeSlug);
  }, { port: runtimePort, activeSlug: slug });

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());
    if (url.pathname === "/api/check-version") {
      await route.fulfill({ json: { ok: true, latest_version: "0.0.0" } });
      return;
    }
    if (url.hostname !== "127.0.0.1" || url.port !== String(runtimePort)) {
      await route.continue();
      return;
    }

    if (url.pathname === "/health") {
      await route.fulfill({ json: { service: "gitim-runtime", version: "0.0.0" } });
      return;
    }
    if (url.pathname === "/workspaces") {
      await route.fulfill({
        json: {
          workspaces: [
            {
              slug,
              workspace_name: "Mobile",
              path: "/tmp/mobile",
              provider: "local",
              initialized: true,
            },
          ],
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/me`) {
      await route.fulfill({ json: { ok: true, data: { handler: "lewis" } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/channels`) {
      await route.fulfill({
        json: {
          ok: true,
          data: {
            channels: [
              { name: "general", kind: "channel", members: ["lewis"] },
              { name: "alice--lewis", kind: "dm", members: ["alice", "lewis"] },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/users`) {
      await route.fulfill({ json: { ok: true, data: { users: ["lewis", "alice"] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/agents`) {
      await route.fulfill({ json: { ok: true, agents: [] } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/cards`) {
      await route.fulfill({ json: { ok: true, data: { cards: [] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/cards/general/card-1`) {
      await route.fulfill({
        json: {
          ok: true,
          data: {
            meta: {
              channel: "general",
              card_id: "card-1",
              title: "Mobile card",
              status: "todo",
              labels: [],
              created_by: "lewis",
              created_at: "20260317T120000Z",
              updated_at: "20260317T120000Z",
            },
            entries: [],
            archived: false,
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/read`) {
      await route.fulfill({
        json: {
          ok: true,
          data: {
            entries: [
              {
                line_number: 1,
                point_to: 0,
                author: "lewis",
                timestamp: "20260317T120000Z",
                body: "hello mobile",
              },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/poll`) {
      await route.fulfill({ json: { ok: true, data: { commit_id: "1", changes: [] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/channels/archived`) {
      await route.fulfill({ json: { ok: true, data: { channels: [] } } });
      return;
    }

    await route.fulfill({ status: 404, json: { ok: false, error: url.pathname } });
  });
}

test("mobile runtime mode defaults to chat", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubRuntime(page);
  await page.goto("/");

  await expect(page).toHaveURL(/\/chat$/);
  await expect(page.getByText("hello mobile")).toBeVisible();
  await expect(page.getByRole("button", { name: "Agents", exact: true })).toHaveCount(0);
});

test("mobile chat uses drawer navigation and bottom tabs", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubRuntime(page);
  await page.goto("/chat");

  await expect(page.getByText("hello mobile")).toBeVisible();
  await expect(page.getByRole("button", { name: "Open conversations" })).toBeVisible();
  await expect(page.getByRole("button", { name: "Chat", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Cards", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Agents", exact: true })).toHaveCount(0);

  const channelCardsButton = page.getByRole("button", { name: "Open cards for general" });
  await expect(channelCardsButton).toContainText("Cards");
  await channelCardsButton.click();
  await expect(page.getByRole("dialog", { name: "Cards in #general" })).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("dialog", { name: "Cards in #general" })).toHaveCount(0);

  await page.getByRole("button", { name: "Open conversations" }).click();
  await expect(page.getByRole("button", { name: "general", exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "alice", exact: true })).toBeVisible();
});

test("mobile card detail uses the shared bottom tabs once", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubRuntime(page);
  await page.goto("/cards/general/card-1");

  await expect(page.getByText("Mobile card")).toBeVisible();
  await expect(page.getByRole("button", { name: "Chat", exact: true })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Cards", exact: true })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Agents", exact: true })).toHaveCount(0);
});

test("browser mode setup is reachable without a runtime port", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
  });
  await page.goto("/");

  await expect(page.getByText("Browser Mode")).toBeVisible();
  await expect(page.getByLabel("Git remote URL")).toBeVisible();
  await expect(page.getByLabel("Personal access token")).toBeVisible();
  await expect(page.getByLabel("Handler")).toHaveCount(0);
  await expect(page.getByRole("button", { name: "Connect" })).toBeDisabled();
});

test("fresh setup can continue with desktop runtime mode", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.clear();
  });
  await page.goto("/");

  await expect(page.getByText("Choose Mode")).toBeVisible();
  await page.getByRole("button", { name: /Desktop Runtime/ }).click();

  await expect(page.getByText("Install Runtime")).toBeVisible();
  await page.getByRole("button", { name: "Runtime is running — continue" }).click();

  await expect(page.getByText("Connect Runtime")).toBeVisible();
  await expect(page.getByTestId("port-input")).toBeVisible();
});

test("fresh setup can switch to browser mode from the mode choice", async ({ page }) => {
  await page.addInitScript(() => {
    localStorage.clear();
  });
  await page.goto("/");

  await expect(page.getByText("Choose Mode")).toBeVisible();
  await page.getByRole("button", { name: /Browser Mode/ }).click();

  await expect(page.getByText("Browser Mode")).toBeVisible();
  await expect(page.getByLabel("Git remote URL")).toBeVisible();
});

test("browser mode preflights worker dependencies before clone", async ({ page }) => {
  const pageErrors: string[] = [];
  page.on("pageerror", (error) => pageErrors.push(error.message));

  await page.addInitScript(() => {
    localStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
  });
  await page.route("**/*", async (route) => {
    const url = route.request().url();
    if (url === "https://api.github.com/user") {
      await route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify({ login: "flame4", name: "Flame4", email: null }),
      });
      return;
    }
    if (url.startsWith("https://cors.isomorphic-git.org/")) {
      await route.fulfill({
        status: 500,
        contentType: "text/plain",
        body: "stubbed clone failure",
      });
      return;
    }
    await route.continue();
  });

  await page.goto("/");
  await page.getByLabel("Git remote URL").fill("https://github.com/flame4/room");
  await page.getByLabel("Personal access token").fill("dummy-token");
  await page.getByRole("button", { name: "Connect" }).click();

  await expect(page.getByText("Signed in as @flame4")).toBeVisible();
  await expect(page.getByText("HTTP Error: 500 Internal Server Error")).toBeVisible();
  expect(pageErrors.join("\n")).not.toMatch(/Buffer|TextEncoder|createHash|crypto/);
});
