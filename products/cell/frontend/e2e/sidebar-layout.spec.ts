import { expect, test, type Page } from "@playwright/test";

const runtimePort = 49321;
const slug = "layout";

function agents(count: number) {
  return Array.from({ length: count }, (_, i) => {
    const n = String(i + 1).padStart(2, "0");
    return {
      id: `agent-${n}`,
      handler: `agent-${n}`,
      display_name: `agent-${n}`,
      status: i % 3 === 0 ? "running" : "idle",
      repo_path: "",
      messages_processed: i,
      last_activity: "2026-04-29T10:00:00Z",
    };
  });
}

async function stubRuntime(page: Page) {
  await page.addInitScript(({ port, activeSlug }) => {
    const activities = {
      "agent-01": [
        {
          agent_id: "agent-01",
          event_type: "done",
          detail: "finished setup",
          timestamp: "2026-04-29T10:01:00Z",
        },
      ],
      "agent-04": [
        {
          agent_id: "agent-04",
          event_type: "thinking",
          detail: "reviewing changes",
          timestamp: "2026-04-29T10:03:00Z",
        },
      ],
      "agent-20": [
        {
          agent_id: "agent-20",
          event_type: "tool_use",
          detail: "running tests",
          timestamp: "2026-04-29T10:05:00Z",
        },
      ],
    };

    localStorage.clear();
    localStorage.setItem("gitim-runtime-port", String(port));
    localStorage.setItem("gitim-active-workspace", activeSlug);
    localStorage.setItem(
      "gitim/agent-activity",
      JSON.stringify({ state: { activities, lastSlug: activeSlug }, version: 0 }),
    );
    localStorage.setItem(
      `gitim-known-agents:${activeSlug}`,
      JSON.stringify(["agent-01", "agent-04", "agent-20", "agent-30"]),
    );
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
              workspace_name: "Layout",
              path: "/tmp/layout",
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
              {
                name: "general",
                kind: "channel",
                members: ["lewis"],
              },
              {
                name: "ops",
                kind: "channel",
                members: ["lewis"],
              },
              {
                name: "agent-01--lewis",
                kind: "dm",
                members: ["agent-01", "lewis"],
              },
              {
                name: "agent-30--lewis",
                kind: "dm",
                members: ["agent-30", "lewis"],
              },
              {
                name: "alice--lewis",
                kind: "dm",
                members: ["alice", "lewis"],
              },
              {
                name: "agent-04--agent-30",
                kind: "dm",
                members: ["agent-04", "agent-30"],
              },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/users`) {
      await route.fulfill({
        json: {
          ok: true,
          data: { users: ["lewis", "alice", "agent-01", "agent-30"] },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/agents`) {
      await route.fulfill({ json: { ok: true, agents: agents(24) } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/cards`) {
      await route.fulfill({ json: { ok: true, data: { cards: [] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/read`) {
      await route.fulfill({ json: { ok: true, data: { entries: [] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/poll`) {
      await route.fulfill({ json: { ok: true, data: { commit_id: "1", changes: [] } } });
      return;
    }

    await route.fulfill({ status: 404, json: { ok: false, error: url.pathname } });
  });
}

test("chat sidebar keeps channels visible when many agents are active", async ({ page }) => {
  await stubRuntime(page);
  await page.goto("/chat");

  const sidebar = page.locator(".w-64").first();
  const channelsHeading = page.getByText("Channels", { exact: true });
  const previewRows = page.getByTestId("agent-preview-row");

  await expect(channelsHeading).toBeVisible();
  await expect(previewRows).toHaveCount(3);
  await expect(previewRows.nth(0)).toContainText("agent-20");
  await expect(previewRows.nth(0)).toContainText("running tests");

  const [sidebarBox, channelsBox] = await Promise.all([
    sidebar.boundingBox(),
    channelsHeading.boundingBox(),
  ]);

  expect(sidebarBox).not.toBeNull();
  expect(channelsBox).not.toBeNull();
  expect(channelsBox!.y).toBeGreaterThanOrEqual(sidebarBox!.y);
  expect(channelsBox!.y + channelsBox!.height).toBeLessThanOrEqual(
    sidebarBox!.y + sidebarBox!.height,
  );
  await expect(page.getByRole("button", { name: "general" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "agent-01" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "alice" })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "agent-30" })).toHaveCount(0);
  await expect(sidebar.getByRole("button", { name: "agent-04 ↔ agent-30" })).toHaveCount(0);

  await page.getByRole("button", { name: "Show all agents" }).click();
  await expect(page.getByTestId("agent-full-row")).toHaveCount(24);
  await expect(page.getByTestId("agent-full-row").nth(0)).toContainText("agent-20");
});
