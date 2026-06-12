/**
 * E2E test: channel-project sidebar feature
 *
 * Tests project folders in the sidebar: project renders as collapsible folder,
 * channels within a project are hidden when collapsed and visible when expanded,
 * pin persists to localStorage and survives a full page reload.
 *
 * Implementation follows sidebar-layout.spec.ts pattern:
 *   - addInitScript to seed localStorage (port + active workspace)
 *   - page.route to stub all runtime HTTP calls
 *
 * Skipped scenario: "send message → routing unaffected"
 *   Routing is enforced by the daemon (recipients field on poll entries).
 *   Against stubs this would always succeed vacuously. Covered by daemon
 *   integration tests in gitim-daemon. See design §5 (routing untouched by
 *   project assignment).
 */
import { expect, test, type Page } from "@playwright/test";

const runtimePort = 49322; // distinct from sidebar-layout.spec.ts (49321)
const slug = "proj";

async function stubRuntime(page: Page) {
  await page.addInitScript(
    ({ port, activeSlug }) => {
      const workspaceKey = `runtime:${activeSlug}`;
      // Preserve pinned state across reloads — addInitScript runs before every
      // page navigation, so we must re-apply whatever localStorage held from
      // the previous page load. sidebar-layout.spec.ts uses the same pattern.
      const pinnedConversations = localStorage.getItem(
        `gitim-pinned-conversations:${workspaceKey}`,
      );
      const expandedProjects = localStorage.getItem(
        `gitim-expanded-projects:${workspaceKey}`,
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
      if (expandedProjects) {
        localStorage.setItem(
          `gitim-expanded-projects:${workspaceKey}`,
          expandedProjects,
        );
      }
    },
    { port: runtimePort, activeSlug: slug },
  );

  await page.route("**/*", async (route) => {
    const url = new URL(route.request().url());

    if (url.pathname === "/api/check-version") {
      await route.fulfill({ json: { ok: true, latest_version: "0.0.0" } });
      return;
    }
    // Only intercept our stub runtime; let everything else through (Vite HMR, etc.)
    if (url.hostname !== "127.0.0.1" || url.port !== String(runtimePort)) {
      await route.continue();
      return;
    }

    if (url.pathname === "/health") {
      await route.fulfill({
        json: { service: "gitim-runtime", version: "0.0.0" },
      });
      return;
    }
    if (url.pathname === "/workspaces") {
      await route.fulfill({
        json: {
          workspaces: [
            {
              slug,
              workspace_name: "Proj",
              path: "/tmp/proj",
              provider: "local",
              initialized: true,
            },
          ],
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/me`) {
      await route.fulfill({
        json: { ok: true, data: { handler: "lewis" } },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/channels`) {
      // Two channels:
      //   - "dev"    → assigned to project "design"
      //   - "random" → unassigned (no project)
      await route.fulfill({
        json: {
          ok: true,
          data: {
            channels: [
              {
                name: "dev",
                kind: "channel",
                members: ["lewis"],
                project: "design",
              },
              {
                name: "random",
                kind: "channel",
                members: ["lewis"],
              },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/projects`) {
      // One project: "design" with display_name "Design", channel_count 1.
      // Wire shape: { ok: true, data: { projects: [...] } }
      await route.fulfill({
        json: {
          ok: true,
          data: {
            projects: [
              {
                slug: "design",
                meta: {
                  display_name: "Design",
                  created_by: "lewis",
                  created_at: "2026-01-01T00:00:00Z",
                  introduction: "Design project",
                },
                channel_count: 1,
              },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/users`) {
      await route.fulfill({
        json: { ok: true, data: { users: ["lewis"] } },
      });
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
    if (url.pathname === `/workspaces/${slug}/im/read`) {
      await route.fulfill({
        json: { ok: true, data: { entries: [] } },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/poll`) {
      await route.fulfill({
        json: { ok: true, data: { commit_id: "1", changes: [] } },
      });
      return;
    }

    await route.fulfill({
      status: 404,
      json: { ok: false, error: url.pathname },
    });
  });
}

// ---------------------------------------------------------------------------
// Scenario 1: project folder visible, unassigned channel at top level,
//             assigned channel hidden by default (folder collapsed),
//             expand → channel appears, click channel → active styling
// ---------------------------------------------------------------------------
test("project folder collapsed by default; expand reveals child channel", async ({
  page,
}) => {
  await stubRuntime(page);
  await page.goto("/chat");

  // "Design" project folder must appear in the sidebar
  const projectHeader = page.getByRole("button", {
    name: "Project Design",
    exact: true,
  });
  await expect(projectHeader).toBeVisible();

  // "random" is unassigned → rendered as a standalone channel item at the top level
  await expect(
    page.getByRole("button", { name: "random", exact: true }),
  ).toBeVisible();

  // "dev" is inside the collapsed "Design" project → must NOT be visible yet
  const projectChildren = page.getByTestId("sidebar-project-children");
  await expect(projectChildren).toHaveCount(0);

  // Click the project header to expand it.
  // The inner <button> has stopPropagation so only one handler fires.
  await page
    .getByTestId("sidebar-project-header")
    .locator("button")
    .first()
    .click();

  // "dev" should now appear inside the project children area
  await expect(page.getByTestId("sidebar-project-children")).toBeVisible();
  await expect(
    page.getByTestId("sidebar-project-channel-item").filter({ hasText: "dev" }),
  ).toBeVisible();

  // Click "dev" → channel becomes the active selection in Zustand store.
  // Channel navigation is Zustand state, not URL state (URL stays at /chat).
  // We verify the click registers by checking the active CSS class on the <li>.
  const devItem = page
    .getByTestId("sidebar-project-channel-item")
    .filter({ hasText: "dev" });

  // Click the named "dev" button (the channel label button, not pin/fold buttons)
  await page.getByRole("button", { name: "dev", exact: true }).click();

  // Active ChannelItem gets the bg-primary/15 CSS class. We verify via a
  // lightweight class check — no aria-current is emitted by ChannelItem.
  await expect(devItem).toHaveClass(/bg-primary/);
});

// ---------------------------------------------------------------------------
// Scenario 2: pin project → localStorage written correctly
//             page.reload() → project still pinned (listed first / pin indicator)
//             AND expand state persists
// ---------------------------------------------------------------------------
test("pin project persists through reload", async ({ page }) => {
  await stubRuntime(page);
  await page.goto("/chat");

  // Wait for "Design" project folder to be present
  const projectHeader = page.getByRole("button", {
    name: "Project Design",
    exact: true,
  });
  await expect(projectHeader).toBeVisible();

  // Expand the project first so we can verify collapsed/expanded state persists too.
  await page
    .getByTestId("sidebar-project-header")
    .locator("button")
    .first()
    .click();
  await expect(page.getByTestId("sidebar-project-children")).toBeVisible();

  // Hover over the project header to reveal the pin button
  // (pin button is opacity-0 by default, group-hover:opacity-100)
  await page.getByTestId("sidebar-project-header").hover();

  // Click the pin button — label: "Pin project Design"
  await page.getByRole("button", { name: "Pin project Design", exact: true }).click();

  // Verify localStorage was written with projects: ["design"]
  await expect
    .poll(() =>
      page.evaluate(() =>
        localStorage.getItem("gitim-pinned-conversations:runtime:proj"),
      ),
    )
    .toBe(JSON.stringify({ channels: [], dms: [], projects: ["design"] }));

  // Verify expanded state was also written to localStorage
  await expect
    .poll(() =>
      page.evaluate(() =>
        localStorage.getItem("gitim-expanded-projects:runtime:proj"),
      ),
    )
    .toBe(JSON.stringify(["design"]));

  // Reload the page — addInitScript will re-apply the localStorage values
  await page.reload();

  // "Design" project should still be present and pinned
  const projectHeaderAfterReload = page.getByRole("button", {
    name: "Project Design",
    exact: true,
  });
  await expect(projectHeaderAfterReload).toBeVisible();

  // Pin button should now show the "Unpin" label (aria-pressed=true / pinned state)
  const pinButton = page.getByRole("button", {
    name: "Unpin project Design",
    exact: true,
  });
  await expect(pinButton).toBeVisible();

  // Children should be visible (expanded state restored)
  await expect(page.getByTestId("sidebar-project-children")).toBeVisible();
  await expect(
    page.getByTestId("sidebar-project-channel-item").filter({ hasText: "dev" }),
  ).toBeVisible();
});
