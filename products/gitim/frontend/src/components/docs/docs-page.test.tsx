// @vitest-environment jsdom
import { act } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter, useLocation } from "react-router";
import { DocsPage } from "./docs-page";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function LocationProbe() {
  const location = useLocation();
  return (
    <output data-testid="location-probe">
      {location.pathname}
      {location.search}
    </output>
  );
}

describe("DocsPage", () => {
  let root: Root | null = null;
  const windowScrollTo = vi.fn();

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
      root = null;
    }
    document.body.innerHTML = "";
    windowScrollTo.mockClear();
  });

  async function render(entry = "/docs") {
    window.scrollTo = windowScrollTo;
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root?.render(
        <MemoryRouter initialEntries={[entry]}>
          <DocsPage />
          <LocationProbe />
        </MemoryRouter>,
      );
    });
    return container;
  }

  it("opens with a standard Quick Start flow", async () => {
    const container = await render();

    expect(container.querySelector('[data-testid="docs-page"]')).not.toBeNull();
    expect(
      container.querySelector('[data-testid="docs-heading"]')?.textContent,
    ).toBe("Quick Start");
    expect(container.textContent).toContain("Four steps to a working agent team");
    expect(container.textContent).toContain("gitim.io");
    expect(container.textContent).toContain("./scripts/install-from-source.sh");
    expect(container.textContent).toContain("Choose how GitIM runs");
    expect(container.textContent).toContain("Create or open a workspace");
    expect(container.textContent).toContain("Add your first agent");
    expect(container.textContent).toContain("Start collaborating");
    expect(
      container.querySelectorAll('[data-testid="docs-concept"]'),
    ).toHaveLength(4);
    expect(container.textContent).not.toContain(
      "lightweight chat client that connects to a local GitIM daemon",
    );
  });

  it("covers the current product through grouped documentation chapters", async () => {
    const container = await render();
    const chapters = [
      ["workspaces", "Workspaces & Setup", "A workspace is a Git repository"],
      ["github-token", "GitHub Token", "Contents: Read and write"],
      ["agents", "Agents & Providers", "Provisioning is transactional"],
      ["messaging", "Messaging", "Recipient routing"],
      ["work-management", "Work Management", "Cards, projects, boards, and labels"],
      ["automation", "Flows & Automation", "Templates, runs, schedules, and timers"],
      ["quick-sessions", "Quick Sessions", "Focused agent conversations"],
      ["protocol", "Protocol & Storage", "[L000042][P000000]"],
      ["runtime", "Runtime & Sync", "Runtime → daemon → Git"],
      ["distributed", "Distributed, Browser & Mobile", "Browser and mobile"],
      ["cli-api", "CLI & API", "Two command surfaces"],
      ["operations", "Operations & Security", "Operational boundaries"],
    ] as const;

    expect(
      container.querySelectorAll('[data-testid^="docs-group-"]'),
    ).toHaveLength(4);
    expect(
      container.querySelectorAll('[data-testid^="docs-nav-"]'),
    ).toHaveLength(13);

    for (const [id, title, marker] of chapters) {
      const button = container.querySelector<HTMLButtonElement>(
        `[data-testid="docs-nav-${id}"]`,
      );
      expect(button, id).not.toBeNull();
      await act(async () => {
        button?.click();
      });
      expect(
        container.querySelector('[data-testid="docs-heading"]')?.textContent,
        id,
      ).toBe(title);
      expect(container.textContent, id).toContain(marker);
      expect(
        container.querySelectorAll('[data-testid="docs-concept"]').length,
        `${id} concept coverage`,
      ).toBeGreaterThanOrEqual(4);
      expect(
        container.querySelector('[data-testid="location-probe"]')?.textContent,
        id,
      ).toBe(`/docs?tab=${id}`);
    }
  });

  it("teaches every chapter through a visual, an example, and progressive detail", async () => {
    const container = await render();
    const chapterIds = [
      "quickstart",
      "workspaces",
      "github-token",
      "agents",
      "messaging",
      "work-management",
      "automation",
      "quick-sessions",
      "protocol",
      "runtime",
      "distributed",
      "cli-api",
      "operations",
    ] as const;

    for (const id of chapterIds) {
      if (id !== "quickstart") {
        const chapter = container.querySelector<HTMLButtonElement>(
          `[data-testid="docs-nav-${id}"]`,
        );
        await act(async () => {
          chapter?.click();
        });
      }

      expect(
        container.querySelector('[data-testid="docs-concept-flow"]'),
        `${id} concept flow`,
      ).not.toBeNull();
      expect(
        container.querySelector(`[data-testid="docs-example-${id}"]`),
        `${id} worked example`,
      ).not.toBeNull();
      expect(
        container.querySelector('[data-testid="docs-recorded-artifact"]'),
        `${id} recorded artifact`,
      ).not.toBeNull();

      const concepts = container.querySelector<HTMLDetailsElement>(
        'details[data-testid="docs-concepts"]',
      );
      expect(concepts, `${id} progressive concepts`).not.toBeNull();
      expect(concepts?.open, `${id} concepts start collapsed`).toBe(false);
      const conceptDetails = container.querySelectorAll<HTMLDetailsElement>(
        'details[data-testid="docs-concept"]',
      );
      expect(conceptDetails.length, `${id} nested concepts`).toBeGreaterThanOrEqual(4);
    }
  });

  it("uses one concrete scenario to explain message routing", async () => {
    const container = await render("/docs?tab=messaging");

    expect(container.textContent).toContain(
      "Maya asks @planner to turn a release goal into tracked work.",
    );
    expect(
      container.querySelector('[data-testid="docs-example-messaging"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="docs-recorded-artifact"]')
        ?.textContent,
    ).toContain("channels/launch.thread");
  });

  it("supports deep links and recovers unknown chapter ids", async () => {
    let container = await render("/docs?tab=github-token");
    expect(
      container.querySelector('[data-testid="docs-heading"]')?.textContent,
    ).toBe("GitHub Token");
    expect(
      container
        .querySelector('[data-testid="docs-nav-github-token"]')
        ?.getAttribute("aria-current"),
    ).toBe("page");

    act(() => {
      root?.unmount();
    });
    root = null;
    document.body.innerHTML = "";

    container = await render("/docs?tab=missing");
    expect(
      container.querySelector('[data-testid="docs-heading"]')?.textContent,
    ).toBe("Quick Start");
  });

  it("returns directly to GitIM instead of walking chapter history", async () => {
    const container = await render("/docs?tab=agents");
    const messaging = container.querySelector<HTMLButtonElement>(
      '[data-testid="docs-nav-messaging"]',
    );
    await act(async () => {
      messaging?.click();
    });
    expect(
      container.querySelector('[data-testid="location-probe"]')?.textContent,
    ).toBe("/docs?tab=messaging");

    const backToGitim = Array.from(
      container.querySelectorAll<HTMLButtonElement>("button"),
    ).find((button) => button.textContent?.includes("Back to GitIM"));
    expect(backToGitim).not.toBeNull();
    await act(async () => {
      backToGitim?.click();
    });

    expect(
      container.querySelector('[data-testid="location-probe"]')?.textContent,
    ).toBe("/");
  });

  it("makes every recommended next chapter directly navigable", async () => {
    const container = await render();
    const nextChapters = [
      ["agents", "Agents & Providers"],
      ["messaging", "Messaging"],
      ["work-management", "Work Management"],
      ["automation", "Flows & Automation"],
    ] as const;

    expect(
      container.querySelectorAll('[data-testid^="docs-next-"]'),
    ).toHaveLength(4);

    for (const [id, heading] of nextChapters) {
      const link = container.querySelector<HTMLAnchorElement>(
        `[data-testid="docs-next-${id}"]`,
      );
      expect(link?.getAttribute("href"), id).toBe(`/docs?tab=${id}`);
      await act(async () => {
        link?.click();
      });
      expect(
        container.querySelector('[data-testid="docs-heading"]')?.textContent,
        id,
      ).toBe(heading);

      const quickStart = container.querySelector<HTMLButtonElement>(
        '[data-testid="docs-nav-quickstart"]',
      );
      await act(async () => {
        quickStart?.click();
      });
    }
  });

  it("resets both possible scroll containers when changing chapters", async () => {
    const container = await render();
    const main = container.querySelector("main");
    const scrollTo = vi.fn();
    if (main) main.scrollTo = scrollTo;

    const agents = container.querySelector<HTMLButtonElement>(
      '[data-testid="docs-nav-agents"]',
    );
    await act(async () => {
      agents?.click();
    });

    expect(scrollTo).toHaveBeenCalledWith({ top: 0, behavior: "auto" });
    expect(windowScrollTo).toHaveBeenCalledWith({ top: 0, behavior: "auto" });
  });

  it("resets document scroll when a recommended chapter link changes the URL", async () => {
    const container = await render();
    windowScrollTo.mockClear();

    const agents = container.querySelector<HTMLAnchorElement>(
      '[data-testid="docs-next-agents"]',
    );
    await act(async () => {
      agents?.click();
    });

    expect(windowScrollTo).toHaveBeenCalledWith({ top: 0, behavior: "auto" });
  });
});
