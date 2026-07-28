import { expect, test, type Page } from "@playwright/test";

const runtimePort = 49328;
const slug = "quick-room";

async function stubRuntime(page: Page) {
  let session: Record<string, unknown> | null = null;
  let sessionId = "";
  let archived = false;
  let channelSends = 0;
  let agentReplyReleased = false;
  let agentReplyVisible = false;
  let processedPolls = 0;
  let refreshedLists = 0;
  let refreshedDetails = 0;

  await page.addInitScript(
    ({ port, activeSlug }) => {
      localStorage.clear();
      localStorage.setItem("gitim-runtime-port", String(port));
      localStorage.setItem("gitim-active-workspace", activeSlug);
    },
    { port: runtimePort, activeSlug: slug },
  );

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
              workspace_name: "Quick room",
              path: "/tmp/quick-room",
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
                members: ["lewis", "alice"],
                created_by: "lewis",
              },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/projects`) {
      await route.fulfill({ json: { ok: true, data: { projects: [] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/users`) {
      await route.fulfill({ json: { ok: true, data: { users: ["lewis", "alice"] } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/agents`) {
      await route.fulfill({
        json: {
          ok: true,
          agents: [
            {
              id: "alice",
              handler: "alice",
              display_name: "Alice",
              status: "idle",
              system_prompt: "",
              repo_path: "/tmp/alice",
              messages_processed: 0,
            },
          ],
        },
      });
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
      if (session && agentReplyReleased && !agentReplyVisible) {
        const detail = session as {
          meta: Record<string, unknown>;
          entries: Record<string, unknown>[];
        };
        detail.meta.title = "Investigate flakes";
        detail.meta.status = "active";
        detail.meta.updated_at = "2026-07-11T00:00:02Z";
        detail.meta.last_message_preview = "Ready to investigate";
        detail.meta.revision = 4;
        detail.entries.push({
          line_number: 2,
          point_to: 1,
          author: "alice",
          timestamp: "20260711T000002Z",
          body: "Ready to investigate",
        });
        agentReplyVisible = true;
        processedPolls += 1;
        await route.fulfill({
          json: {
            ok: true,
            data: {
              commit_id: "2",
              changes: [
                { channel: sessionId, kind: "quick_session_meta" },
                { channel: sessionId, kind: "quick_session_thread" },
              ],
            },
          },
        });
        return;
      }
      await route.fulfill({
        json: {
          ok: true,
          data: { commit_id: agentReplyVisible ? "2" : "1", changes: [] },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/send`) {
      channelSends += 1;
      await route.fulfill({ json: { ok: true, data: { line_number: 1 } } });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/quick-sessions`) {
      if (route.request().method() === "POST") {
        const body = route.request().postDataJSON() as {
          session_id: string;
          agent_id: string;
          first_message: string;
        };
        sessionId = body.session_id;
        archived = false;
        session = {
          meta: {
            id: sessionId,
            title: null,
            agent_id: body.agent_id,
            created_by: "lewis",
            status: "needs_title",
            created_at: "2026-07-11T00:00:00Z",
            updated_at: "2026-07-11T00:00:00Z",
            last_message_preview: body.first_message,
            revision: 1,
          },
          entries: [
            {
              line_number: 1,
              point_to: 0,
              author: "lewis",
              timestamp: "20260711T000000Z",
              body: body.first_message,
            },
          ],
          archived: false,
        };
        await route.fulfill({
          json: {
            ok: true,
            data: { session, line_number: 1, ref: `session:${sessionId}` },
          },
        });
        return;
      }
      if (agentReplyVisible) refreshedLists += 1;
      const wantsArchived = url.searchParams.get("archived") === "true";
      const sessions =
        session && wantsArchived === archived
          ? [
              {
                ...(session as { meta: Record<string, unknown> }).meta,
                archived,
                ref: `session:${sessionId}`,
              },
            ]
          : [];
      await route.fulfill({ json: { ok: true, data: { sessions } } });
      return;
    }
    if (sessionId && url.pathname === `/workspaces/${slug}/im/quick-sessions/${sessionId}`) {
      if (agentReplyVisible) refreshedDetails += 1;
      await route.fulfill({ json: { ok: true, data: { session } } });
      return;
    }
    if (
      sessionId &&
      url.pathname === `/workspaces/${slug}/im/quick-sessions/${sessionId}/archive`
    ) {
      archived = true;
      const detail = session as { meta: Record<string, unknown>; archived: boolean };
      detail.archived = true;
      detail.meta.status = "archived";
      detail.meta.revision = 5;
      await route.fulfill({
        json: {
          ok: true,
          data: { session_id: sessionId, status: "archived", revision: 5 },
        },
      });
      return;
    }
    await route.fulfill({ status: 404, json: { ok: false, error: url.pathname } });
  });

  return {
    channelSendCount: () => channelSends,
    releaseAgentReply: () => {
      agentReplyReleased = true;
    },
    processedPollCount: () => processedPolls,
    refreshedListCount: () => refreshedLists,
    refreshedDetailCount: () => refreshedDetails,
  };
}

test("creates, references, and archives a Quick Session", async ({ page }) => {
  const runtime = await stubRuntime(page);
  await page.goto("/chat");
  await page.getByRole("button", { name: "general", exact: true }).click();

  const composer = page.getByPlaceholder(/Type a message/);
  const quickSessions = page.getByRole("button", { name: "Quick Sessions" });
  await composer.focus();
  await expect(composer).toBeFocused();
  await quickSessions.hover();
  await expect(page.getByRole("heading", { name: "Quick Sessions" })).toBeVisible();
  await expect(composer).toBeFocused();
  await composer.hover();
  await page.waitForTimeout(300);
  await expect(page.getByRole("heading", { name: "Quick Sessions" })).toBeHidden();
  await expect(composer).toBeFocused();
  await quickSessions.click();
  await page.getByRole("button", { name: "New Quick Session" }).click();
  await page.getByPlaceholder("What should this session focus on?").fill("Investigate flakes");
  await page.getByRole("button", { name: "Start session" }).click();
  await expect(page.getByRole("heading", { name: "Untitled session" })).toBeVisible();
  await expect(page.getByText("needs_title", { exact: true }).last()).toBeVisible();

  runtime.releaseAgentReply();
  await expect
    .poll(() => runtime.processedPollCount(), { timeout: 10_000 })
    .toBe(1);
  await expect.poll(() => runtime.refreshedListCount()).toBeGreaterThan(0);
  await expect.poll(() => runtime.refreshedDetailCount()).toBeGreaterThan(0);

  await page.keyboard.press("Escape");
  await page.getByRole("button", { name: "Quick Sessions" }).click();
  await expect(page.getByRole("heading", { name: "Investigate flakes" })).toBeVisible({
    timeout: 5_000,
  });
  await expect(page.getByText("Ready to investigate", { exact: true }).last()).toBeVisible();

  const row = page.getByRole("listitem").filter({ hasText: "Investigate flakes" });
  await row.getByRole("button", { name: /Investigate flakes/ }).first().focus();
  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { name: "Quick Sessions" })).toBeHidden();
  await expect(quickSessions).toBeFocused();
  await page.waitForTimeout(100);
  await expect(page.getByRole("heading", { name: "Quick Sessions" })).toBeHidden();
  await quickSessions.click();
  await expect(page.getByRole("heading", { name: "Investigate flakes" })).toBeVisible();

  await row.dragTo(composer);
  await expect(composer).toHaveValue(/^session:qs-/);
  expect(runtime.channelSendCount()).toBe(0);

  await page.keyboard.press("Escape");
  await expect(page.getByRole("heading", { name: "Quick Sessions" })).toBeHidden();
  await page.getByRole("button", { name: "Quick Sessions" }).click();
  await row.getByRole("button", { name: /Investigate flakes/ }).first().click();
  await page.getByRole("button", { name: "Archive session" }).click();
  await page.getByLabel("Show archived").check();
  await expect(page.getByRole("listitem").filter({ hasText: "Investigate flakes" })).toBeVisible();
});
