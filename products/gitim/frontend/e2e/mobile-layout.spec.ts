import { expect, test, type Page } from "@playwright/test";

const runtimePort = 49322;
const slug = "mobile";

interface BrowserWorkspaceRecordFixture {
  id: string;
  slug: string;
  workspace_name: string;
  remoteUrl: string;
  corsProxy: string;
  handler: string;
  storage: { fsName: string; repoDir: string };
  createdAt: string;
  updatedAt: string;
}

function browserWorkspaceRecord(
  id: string,
  workspaceName: string,
  remoteUrl: string,
): BrowserWorkspaceRecordFixture {
  const timestamp = "2026-05-08T12:00:00.000Z";
  return {
    id,
    slug: `browser-${id}`,
    workspace_name: workspaceName,
    remoteUrl,
    corsProxy: "https://cors.isomorphic-git.org",
    handler: "flame4",
    storage: { fsName: `gitim-ws-${id}`, repoDir: "/repo" },
    createdAt: timestamp,
    updatedAt: timestamp,
  };
}

async function preloadBrowserWorkspaces(
  page: Page,
  options: {
    workspaces: BrowserWorkspaceRecordFixture[];
    activeSlug?: string;
    tokens?: Record<string, string>;
  },
) {
  await page.addInitScript(({ workspaces, activeSlug, tokens }) => {
    localStorage.clear();
    sessionStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
    localStorage.setItem(
      "gitim-browser-workspaces-v2",
      JSON.stringify({ version: 2, workspaces }),
    );
    if (activeSlug) {
      localStorage.setItem("gitim-active-browser-workspace", activeSlug);
    }
    for (const [workspaceId, token] of Object.entries(tokens ?? {})) {
      sessionStorage.setItem(`gitim-browser-token:${workspaceId}`, token);
    }
  }, options);
}

async function stubGitHubIdentity(page: Page) {
  await page.route("https://api.github.com/user", async (route) => {
    await route.fulfill({
      status: 200,
      contentType: "application/json",
      body: JSON.stringify({ login: "flame4", name: "Flame4", email: null }),
    });
  });
}

async function stubRuntime(page: Page, sentBodies: Array<Record<string, unknown>> = []) {
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
              { name: "bob--carol", kind: "dm", members: ["bob", "carol"] },
            ],
          },
        },
      });
      return;
    }
    if (url.pathname === `/workspaces/${slug}/im/users`) {
      await route.fulfill({ json: { ok: true, data: { users: ["lewis", "alice", "bob", "carol"] } } });
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
    if (url.pathname === `/workspaces/${slug}/im/send`) {
      const payload = route.request().postDataJSON() as Record<string, unknown>;
      sentBodies.push(payload);
      await route.fulfill({
        json: {
          ok: true,
          data: {
            line_number: 2,
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

async function stubBrowserModeWorker(page: Page) {
  await page.addInitScript(() => {
    type RpcRequest = {
      id: number;
      method: string;
      args: unknown[];
      workspaceId?: string;
      generation?: number;
    };
    type RpcResult = { ok: boolean; data?: unknown; error?: string };
    type Card = {
      card_id: string;
      channel: string;
      title: string;
      status: "todo" | "doing" | "done";
      labels: string[];
      assignee: string | null;
      created_by: string;
      created_at: string;
      updated_at: string;
    };
    type Message = {
      line_number: number;
      point_to: number;
      author: string;
      timestamp: string;
      body: string;
    };

    const cards: Card[] = [];
    const archivedCards: Card[] = [];
    const messagesByCard = new Map<string, Message[]>();

    function handleMethod(method: string, args: unknown[]): RpcResult {
      switch (method) {
        case "preflight":
          return { ok: true, data: { runtime: "browser", storage: "ready", git: "ready" } };
        case "init":
          return { ok: true, data: {} };
        case "startSync":
          return { ok: true };
        case "health":
          return { ok: true, data: { service: "daemon-web", initialized: true } };
        case "me":
          return { ok: true, data: { handler: "flame4", display_name: "Flame4" } };
        case "channels":
          return {
            ok: true,
            data: {
              channels: [
                { name: "general", kind: "channel", unreadCount: 0, members: ["flame4"] },
              ],
            },
          };
        case "users":
          return { ok: true, data: { users: ["flame4"] } };
        case "read":
          return {
            ok: true,
            data: {
              channel: args[0],
              entries: [
                {
                  line_number: 1,
                  point_to: 0,
                  author: "flame4",
                  timestamp: "20260317T120000Z",
                  body: "hello browser cards",
                },
              ],
            },
          };
        case "poll":
          return { ok: true, data: { commit_id: "browser-1", changes: [] } };
        case "thread":
          return { ok: true, data: { entries: [] } };
        case "listCards":
          return { ok: true, data: { cards } };
        case "listArchivedCards":
          return { ok: true, data: { cards: archivedCards } };
        case "createCard": {
          const [channel, title, optsRaw] = args as [
            string,
            string,
            Partial<Card> | undefined,
          ];
          const card: Card = {
            card_id: "20260317-123456-abc",
            channel,
            title,
            status: optsRaw?.status ?? "todo",
            labels: optsRaw?.labels ?? [],
            assignee: optsRaw?.assignee ?? null,
            created_by: "flame4",
            created_at: "20260317T123456Z",
            updated_at: "20260317T123456Z",
          };
          cards.splice(0, cards.length, card);
          messagesByCard.set(`${channel}/${card.card_id}`, []);
          return {
            ok: true,
            data: { channel, card_id: card.card_id, title },
          };
        }
        case "readCard": {
          const [channel, cardId] = args as [string, string];
          const active = cards.find((c) => c.channel === channel && c.card_id === cardId);
          const archived = archivedCards.find((c) => c.channel === channel && c.card_id === cardId);
          const card = active ?? archived;
          if (!card) return { ok: false, error: "card not found" };
          return {
            ok: true,
            data: {
              channel,
              card_id: cardId,
              archived: !!archived,
              meta: card,
              entries: messagesByCard.get(`${channel}/${cardId}`) ?? [],
            },
          };
        }
        case "sendCardMessage": {
          const [channel, cardId, body, replyTo] = args as [
            string,
            string,
            string,
            number | undefined,
          ];
          const key = `${channel}/${cardId}`;
          const existing = messagesByCard.get(key) ?? [];
          const line_number = existing.length + 1;
          const next = [
            ...existing,
            {
              line_number,
              point_to: replyTo ?? 0,
              author: "flame4",
              timestamp: "20260317T123500Z",
              body,
            },
          ];
          messagesByCard.set(key, next);
          return { ok: true, data: { line_number, channel, card_id: cardId } };
        }
        case "updateCard": {
          const [channel, cardId, patch] = args as [string, string, Partial<Card>];
          const card = cards.find((c) => c.channel === channel && c.card_id === cardId);
          if (!card) return { ok: false, error: "card not found" };
          Object.assign(card, patch, { updated_at: "20260317T123600Z" });
          return {
            ok: true,
            data: {
              channel,
              card_id: cardId,
              status: card.status,
              labels: card.labels,
              assignee: card.assignee,
            },
          };
        }
        case "archiveCard": {
          const [channel, cardId] = args as [string, string];
          const idx = cards.findIndex((c) => c.channel === channel && c.card_id === cardId);
          if (idx < 0) return { ok: false, error: "card not found" };
          const [card] = cards.splice(idx, 1);
          archivedCards.splice(0, archivedCards.length, card);
          return { ok: true, data: { channel, card_id: cardId, archived_by: "flame4" } };
        }
        case "unarchiveCard": {
          const [channel, cardId] = args as [string, string];
          const idx = archivedCards.findIndex((c) => c.channel === channel && c.card_id === cardId);
          if (idx < 0) return { ok: false, error: "card not found" };
          const [card] = archivedCards.splice(idx, 1);
          cards.splice(0, cards.length, card);
          return { ok: true, data: { channel, card_id: cardId, unarchived_by: "flame4" } };
        }
        default:
          return { ok: false, error: `unexpected worker method: ${method}` };
      }
    }

    class StubWorker extends EventTarget {
      onmessage: ((event: MessageEvent) => void) | null = null;
      onerror: ((event: ErrorEvent) => void) | null = null;
      onmessageerror: ((event: MessageEvent) => void) | null = null;

      postMessage(raw: unknown): void {
        const request = raw as RpcRequest;
        const result = handleMethod(request.method, request.args);
        queueMicrotask(() => {
          this.onmessage?.(
            new MessageEvent("message", {
              data: {
                id: request.id,
                result,
                workspaceId: request.workspaceId,
                generation: request.generation,
              },
            }),
          );
        });
      }

      terminate(): void {
        return;
      }
    }

    Object.defineProperty(window, "Worker", {
      configurable: true,
      writable: true,
      value: StubWorker,
    });
  });
}

test("mobile runtime mode defaults to chat", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await stubRuntime(page);
  await page.goto("/");

  await expect(page).toHaveURL(/\/chat$/);
  await expect
    .poll(() =>
      page.locator("body > div > div").evaluate((el) => (el as HTMLElement).style.height),
    )
    .toBe("100dvh");
  await expect(page.getByText("hello mobile")).toBeVisible();
  await expect(page.locator("header").getByText("@lewis", { exact: true })).toBeVisible();
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
  const drawer = page.locator(".fixed.inset-0.z-50").first();
  await expect(drawer.getByRole("button", { name: "general", exact: true })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "alice", exact: true })).toBeVisible();
  await expect(drawer.getByText("Others", { exact: true })).toBeVisible();
  await expect(drawer.getByRole("button", { name: "bob ↔ carol", exact: true })).toBeVisible();
});

test("mobile chat Enter inserts newline and send button sends", async ({ page }) => {
  const sentBodies: Array<Record<string, unknown>> = [];
  await page.setViewportSize({ width: 390, height: 844 });
  await stubRuntime(page, sentBodies);
  await page.goto("/chat");

  const input = page.getByPlaceholder("Type a message...");
  await input.fill("hello");
  await input.press("Enter");
  await input.type("world");

  await expect(input).toHaveValue("hello\nworld");
  expect(sentBodies).toHaveLength(0);

  await page.getByRole("button", { name: "Send message" }).click();
  await expect.poll(() => sentBodies.length).toBe(1);
  expect(sentBodies[0]).toMatchObject({
    channel: "general",
    body: "hello\nworld",
  });
  await expect(input).toHaveValue("");
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

test("browser mode refresh reopens the same workspace from session token", async ({ page }) => {
  await page.setViewportSize({ width: 900, height: 844 });
  await page.addInitScript(() => {
    if (sessionStorage.getItem("gitim-e2e-browser-refresh-ready")) return;
    localStorage.clear();
    sessionStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
    sessionStorage.setItem("gitim-e2e-browser-refresh-ready", "1");
  });
  await stubBrowserModeWorker(page);
  await stubGitHubIdentity(page);

  await page.goto("/");
  await page.getByLabel("Workspace name").fill("Phone");
  await page.getByLabel("Git remote URL").fill("https://github.com/flame4/phone");
  await page.getByLabel("Personal access token").fill("dummy-token");
  await page.getByRole("button", { name: "Connect" }).click();

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await page.reload();

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Phone");
  await expect(page.getByLabel("Personal access token")).toHaveCount(0);
});

test("browser mode can switch between registered mobile workspaces", async ({ page }) => {
  const phone = browserWorkspaceRecord(
    "ws_phone",
    "Phone",
    "https://github.com/flame4/phone",
  );
  const tablet = browserWorkspaceRecord(
    "ws_tablet",
    "Tablet",
    "https://github.com/flame4/tablet",
  );

  await page.setViewportSize({ width: 900, height: 844 });
  await preloadBrowserWorkspaces(page, {
    workspaces: [phone, tablet],
    activeSlug: phone.slug,
    tokens: {
      [phone.id]: "phone-token",
      [tablet.id]: "tablet-token",
    },
  });
  await stubBrowserModeWorker(page);

  await page.goto("/");

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Phone");

  await page.getByTestId("workspace-switcher-trigger").click();
  await page.getByTestId(`workspace-row-${tablet.slug}`).click();

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByTestId("workspace-switcher-trigger")).toContainText("Tablet");
});

test("browser mode asks to reconnect when registered workspace has no session token", async ({ page }) => {
  const phone = browserWorkspaceRecord(
    "ws_phone",
    "Phone",
    "https://github.com/flame4/phone",
  );

  await page.setViewportSize({ width: 390, height: 844 });
  await preloadBrowserWorkspaces(page, {
    workspaces: [phone],
    activeSlug: phone.slug,
  });

  await page.goto("/");

  await expect(page.getByText("Browser Mode")).toBeVisible();
  await expect(page.getByText("Phone", { exact: true })).toBeVisible();
  await expect(page.getByRole("button", { name: "Reconnect" })).toBeVisible();

  await page.getByRole("button", { name: "Reconnect" }).click();
  await expect(page.getByLabel("Personal access token")).toBeVisible();
});

test("browser mode mobile app keeps the Cards tab after connecting", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
  });
  await stubBrowserModeWorker(page);
  await stubGitHubIdentity(page);

  await page.goto("/");
  await page.getByLabel("Git remote URL").fill("https://github.com/flame4/room");
  await page.getByLabel("Personal access token").fill("dummy-token");
  await page.getByRole("button", { name: "Connect" }).click();

  await expect(page.getByText("hello browser cards")).toBeVisible();
  await expect(page.getByRole("button", { name: "Chat", exact: true })).toBeVisible();
  const cardsTab = page.getByRole("button", { name: "Cards", exact: true });
  await expect(cardsTab).toBeVisible();
  await expect(page.getByRole("button", { name: "Agents", exact: true })).toHaveCount(0);

  await cardsTab.click();
  await expect(page).toHaveURL(/\/cards$/);
  await expect(page.getByRole("heading", { name: "Cards" })).toBeVisible();
  await expect(page.getByText("No cards yet")).toBeVisible();
});

test("browser mode mobile cards can be created and discussed", async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.addInitScript(() => {
    localStorage.clear();
    localStorage.setItem("gitim-connection-mode", "local");
  });
  await stubBrowserModeWorker(page);
  await stubGitHubIdentity(page);

  await page.goto("/");
  await page.getByLabel("Git remote URL").fill("https://github.com/flame4/room");
  await page.getByLabel("Personal access token").fill("dummy-token");
  await page.getByRole("button", { name: "Connect" }).click();

  await page.getByRole("button", { name: "Cards", exact: true }).click();
  await page.getByRole("button", { name: "New card" }).click();
  await page.getByLabel("Title").fill("Browser card task");
  await page.getByLabel("Channel").selectOption("general");
  await page.getByRole("button", { name: "Create card" }).click();

  await expect(page).toHaveURL(/\/cards\/general\/20260317-123456-abc$/);
  await expect(page.getByRole("heading", { name: "Browser card task" })).toBeVisible();

  const noteInput = page.getByPlaceholder("Write a note");
  await noteInput.fill("first browser note");
  await page.getByRole("button", { name: "Send message" }).click();
  await expect(page.getByText("first browser note")).toBeVisible();

  await page.getByRole("button", { name: "Archive" }).click();
  await expect(page).toHaveURL(/\/cards$/);
  await expect(page.getByText("No cards yet")).toBeVisible();

  await page.evaluate(() => {
    window.history.pushState({}, "", "/cards/general/20260317-123456-abc");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  await expect(page.getByText("This card is archived. Edits are disabled.")).toBeVisible();
  await page.getByRole("button", { name: "Unarchive" }).click();
  await expect(page.getByRole("button", { name: "Archive" })).toBeVisible();
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
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const registry = JSON.parse(
          localStorage.getItem("gitim-browser-workspaces-v2") ??
            '{"workspaces":[]}',
        ) as { workspaces?: unknown[] };
        const tokenKeys = Array.from({ length: sessionStorage.length }, (_, index) =>
          sessionStorage.key(index),
        ).filter((key) => key?.startsWith("gitim-browser-token:"));

        return {
          workspaces: registry.workspaces?.length ?? 0,
          tokenKeys: tokenKeys.length,
        };
      }),
    )
    .toEqual({ workspaces: 0, tokenKeys: 0 });
  expect(pageErrors.join("\n")).not.toMatch(/Buffer|TextEncoder|createHash|crypto/);
});
