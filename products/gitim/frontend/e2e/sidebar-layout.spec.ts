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

    const workspaceKey = `runtime:${activeSlug}`;
    const pinnedConversations = localStorage.getItem(
      `gitim-pinned-conversations:${workspaceKey}`,
    );
    localStorage.clear();
    localStorage.setItem("gitim-runtime-port", String(port));
    localStorage.setItem("gitim-active-workspace", activeSlug);
    if (pinnedConversations) {
      localStorage.setItem(
        `gitim-pinned-conversations:${workspaceKey}`,
        pinnedConversations,
      );
    }
    localStorage.setItem(
      "gitim/agent-activity",
      JSON.stringify({ state: { activities, lastSlug: activeSlug }, version: 0 }),
    );
    localStorage.setItem(
      `gitim-known-agents:${workspaceKey}`,
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
  await expect(page.getByRole("button", { name: "general", exact: true })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "agent-01", exact: true })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "alice", exact: true })).toBeVisible();
  await expect(sidebar.getByRole("button", { name: "agent-30", exact: true })).toHaveCount(0);
  await expect(sidebar.getByRole("button", { name: "agent-04 ↔ agent-30", exact: true })).toHaveCount(0);

  await page.getByRole("button", { name: "Show all agents" }).click();
  await expect(page.getByTestId("agent-full-row")).toHaveCount(24);
  await expect(page.getByTestId("agent-full-row").nth(0)).toContainText("agent-20");
});

test("chat sidebar keeps every section inside the viewport when content overflows", async ({ page }) => {
  await stubRuntime(page);
  // Squeeze viewport vertically so sidebar sections compete for space.
  // Without the fix, the sidebar root has no resolved height — sections
  // expand to their natural height and the DMs block gets pushed below
  // the viewport and clipped by the parent's overflow-hidden.
  await page.setViewportSize({ width: 1280, height: 420 });
  await page.goto("/chat");

  const sidebar = page.locator(".w-64").first();
  const dmsHeading = page.getByText("Direct Messages", { exact: true });

  await expect(dmsHeading).toBeVisible();

  const [sidebarBox, dmsBox] = await Promise.all([
    sidebar.boundingBox(),
    dmsHeading.boundingBox(),
  ]);
  const viewport = page.viewportSize();

  expect(sidebarBox).not.toBeNull();
  expect(dmsBox).not.toBeNull();
  expect(viewport).not.toBeNull();

  // Sidebar fills the viewport vertically — no taller, no shorter.
  expect(sidebarBox!.height).toBeLessThanOrEqual(viewport!.height);
  // DMs heading sits inside the sidebar, not pushed below it.
  expect(dmsBox!.y).toBeGreaterThanOrEqual(sidebarBox!.y);
  expect(dmsBox!.y + dmsBox!.height).toBeLessThanOrEqual(
    sidebarBox!.y + sidebarBox!.height,
  );
});

test("chat sidebar pins channels and direct messages per workspace", async ({ page }) => {
  await stubRuntime(page);
  await page.goto("/chat");

  await expect(page.getByTestId("sidebar-channel-item")).toHaveText([
    "general",
    "ops",
  ]);
  await expect(page.getByTestId("sidebar-dm-item")).toHaveText([
    "agent-01",
    "alice",
  ]);

  await page
    .getByTestId("sidebar-channel-item")
    .filter({ hasText: "ops" })
    .hover();
  await page.getByRole("button", { name: "Pin #ops" }).click();

  await page
    .getByTestId("sidebar-dm-item")
    .filter({ hasText: "alice" })
    .hover();
  await page.getByRole("button", { name: "Pin DM alice" }).click();

  await expect(page.getByTestId("sidebar-channel-item")).toHaveText([
    "ops",
    "general",
  ]);
  await expect(page.getByTestId("sidebar-dm-item")).toHaveText([
    "alice",
    "agent-01",
  ]);

  await expect
    .poll(() =>
      page.evaluate(() =>
        localStorage.getItem("gitim-pinned-conversations:runtime:layout"),
      ),
    )
    .toBe(JSON.stringify({ channels: ["ops"], dms: ["alice--lewis"] }));

  await page.reload();

  await expect(page.getByTestId("sidebar-channel-item")).toHaveText([
    "ops",
    "general",
  ]);
  await expect(page.getByTestId("sidebar-dm-item")).toHaveText([
    "alice",
    "agent-01",
  ]);
});
