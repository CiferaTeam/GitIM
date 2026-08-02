// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { LandingPage } from "./landing-page";
import { incidentScenario } from "@/lib/demo-story";
import messageGraphicSource from "@/assets/gitim-hero-a-message-is-commit.svg?raw";
import repositoryGraphicSource from "@/assets/gitim-hero-b-repo-is-organization.svg?raw";

const mocks = vi.hoisted(() => ({
  setMode: vi.fn(),
  navigate: vi.fn(),
  scrollTo: vi.fn(),
}));

vi.mock("@/hooks/use-connection-store", () => ({
  useConnectionStore: (selector: (s: { setMode: typeof mocks.setMode }) => unknown) =>
    selector({ setMode: mocks.setMode }),
}));

vi.mock("react-router", async () => {
  const actual = await vi.importActual<typeof import("react-router")>("react-router");
  return {
    ...actual,
    useNavigate: () => mocks.navigate,
  };
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function installReducedMotion() {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    configurable: true,
    value: (query: string) => ({
      matches: query.includes("prefers-reduced-motion"),
      media: query,
      onchange: null,
      addEventListener: vi.fn(),
      removeEventListener: vi.fn(),
      addListener: vi.fn(),
      removeListener: vi.fn(),
      dispatchEvent: vi.fn(),
    }),
  });
}

describe("LandingPage PPT demo", () => {
  let root: Root | null = null;

  beforeEach(() => {
    mocks.setMode.mockClear();
    mocks.navigate.mockClear();
    mocks.scrollTo.mockClear();
    Object.defineProperty(HTMLElement.prototype, "scrollTo", {
      configurable: true,
      value: mocks.scrollTo,
    });
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
      root = null;
    }
    document.body.innerHTML = "";
  });

  async function render() {
    const container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root?.render(
        <MemoryRouter>
          <LandingPage />
        </MemoryRouter>,
      );
    });
    return container;
  }

  it("flips between the overview and demo faces", async () => {
    const container = await render();
    const flipCard = container.querySelector<HTMLElement>(
      '[data-testid="landing-flip-card"]',
    );
    expect(container.textContent).toContain("You shape the team");
    expect(container.textContent).toContain("Connect your runtime");
    expect(container.textContent).toContain("Natural as messaging");
    expect(container.textContent).toContain("Auditable in Git");
    expect(container.textContent).toContain("Your data, your repository");
    expect(flipCard?.dataset.side).toBe("overview");
    const overview = container.querySelector<HTMLElement>(
      '[data-testid="landing-first-stage"]',
    );
    expect(overview).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-process"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="demo-stage"]')).toBeNull();

    const cta = container.querySelector(
      '[data-testid="landing-cta-demo"]',
    ) as HTMLButtonElement | null;
    expect(cta).not.toBeNull();
    await act(async () => {
      cta?.click();
    });

    expect(container.querySelector('[data-testid="demo-stage"]')).not.toBeNull();
    expect(flipCard?.dataset.side).toBe("demo");
    expect(flipCard?.className).toContain("[transform:rotateY(180deg)]");
    expect(
      container
        .querySelector('[data-testid="landing-first-stage"]')
        ?.getAttribute("aria-hidden"),
    ).toBe("true");
    const close = container.querySelector(
      '[data-testid="demo-close"]',
    ) as HTMLButtonElement | null;
    expect(close).not.toBeNull();
    expect(close?.textContent).toContain("Back to overview");
    await act(async () => {
      close?.click();
    });
    expect(flipCard?.dataset.side).toBe("overview");
    expect(flipCard?.className).toContain("[transform:rotateY(0deg)]");
    expect(
      container
        .querySelector('[data-testid="landing-first-stage"]')
        ?.getAttribute("aria-hidden"),
    ).toBe("false");
  });

  it("renders the positioning line at subheadline scale", async () => {
    const container = await render();
    const eyebrow = container.querySelector<HTMLElement>(
      '[data-testid="landing-eyebrow"]',
    );

    expect(eyebrow).not.toBeNull();
    expect(eyebrow?.className).toContain("text-lg");
    expect(eyebrow?.className).toContain("tracking-normal");
  });

  it("distributes the intro story across the first viewport", async () => {
    const container = await render();
    const stage = container.querySelector<HTMLElement>(
      '[data-testid="landing-hero-stage"]',
    );
    const copy = container.querySelector<HTMLElement>(
      '[data-testid="landing-hero-copy"]',
    );
    const process = container.querySelector<HTMLElement>(
      '[data-testid="landing-process"]',
    );

    expect(stage?.className).toContain("h-full");
    expect(stage?.className).toContain("max-w-7xl");
    expect(stage?.className).toContain("flex-col");
    expect(copy?.className).toContain("max-w-5xl");
    expect(process?.className).toContain("mt-auto");
    expect(process?.className).toContain("md:mb-12");
  });

  it("renders the six-screen product story", async () => {
    const container = await render();
    const screens = container.querySelectorAll(
      '[data-testid^="landing-screen-"]',
    );
    const messageDiagram = container.querySelector<HTMLImageElement>(
      '[data-testid="landing-product-message"]',
    );
    const repositoryDiagram = container.querySelector<HTMLImageElement>(
      '[data-testid="landing-product-repository"]',
    );
    const messageGrid = container.querySelector<HTMLElement>(
      '[data-testid="landing-product-grid-message"]',
    );
    const repositoryGrid = container.querySelector<HTMLElement>(
      '[data-testid="landing-product-grid-repository"]',
    );
    const messageFrame = container.querySelector<HTMLElement>(
      '[data-testid="landing-product-frame-message"]',
    );

    expect(screens).toHaveLength(6);
    expect(container.querySelector('[data-testid="landing-screen-intro"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-screen-messages"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-screen-repository"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-screen-cards"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-screen-workflow"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-screen-distributed"]')).not.toBeNull();
    expect(messageDiagram?.alt).toBe(
      "A GitIM conversation represented as plain text and Git commits",
    );
    expect(messageDiagram?.getAttribute("src")).toContain(
      "gitim-hero-a-message-is-commit.svg",
    );
    expect(messageDiagram?.getAttribute("loading")).toBe("eager");
    expect(messageDiagram?.className).toContain("h-full");
    expect(messageDiagram?.className).toContain("object-contain");
    expect(repositoryDiagram?.alt).toBe(
      "A GitIM organization represented as a repository of agents, channels, cards, and flows",
    );
    expect(repositoryDiagram?.getAttribute("src")).toContain(
      "gitim-hero-b-repo-is-organization.svg",
    );
    expect(repositoryDiagram?.getAttribute("loading")).toBe("eager");
    expect(repositoryDiagram?.className).toContain("h-full");
    expect(messageGrid?.className).toContain("max-w-[112rem]");
    expect(messageGrid?.className).toContain("xl:grid-cols-[0.5fr_1.5fr]");
    expect(repositoryGrid?.className).toContain("max-w-[112rem]");
    expect(repositoryGrid?.className).toContain("xl:grid-cols-[1.5fr_0.5fr]");
    expect(messageFrame?.className).toContain("aspect-video");
    expect(messageFrame?.className).toContain("w-full");
    expect(container.querySelector('[data-testid="landing-card-board"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-card-wh-3a90"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-card-wh-3a91"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-card-wh-3a92"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-workflow"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-flow-node-coordinator"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-flow-node-verify"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-distributed-network"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-node-server"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-node-browser"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="landing-node-phone"]')).not.toBeNull();
    expect(container.textContent).toContain("Mobile WASM");
    expect(container.textContent).toContain("Start locally by choosing a folder");
    expect(container.textContent).toContain("No GitIM service to deploy");
    expect(
      container.querySelector('[data-testid="landing-local-setup"]')?.textContent,
    ).toContain("choose a folder");
    expect(
      container.querySelector('[data-testid="landing-distributed-setup"]')
        ?.textContent,
    ).toContain("repo + agent env");
    const distributedMap = container.querySelector<HTMLElement>(
      '[data-testid="landing-distributed-map"]',
    );
    const distributedMapClasses = distributedMap?.className.split(/\s+/) ?? [];
    expect(distributedMapClasses).toContain("h-56");
    expect(distributedMapClasses).toContain("lg:h-72");
  });

  it("uses a stronger brand header and a six-step story navigator", async () => {
    const container = await render();
    const header = container.querySelector<HTMLElement>(
      '[data-testid="landing-header"]',
    );
    const brand = container.querySelector<HTMLElement>(
      '[data-testid="landing-brand"]',
    );
    const progress = container.querySelectorAll(
      '[data-testid^="landing-progress-"]',
    );
    const intro = container.querySelector<HTMLElement>(
      '[data-testid="landing-progress-intro"]',
    );
    const distributed = container.querySelector<HTMLButtonElement>(
      '[data-testid="landing-progress-distributed"]',
    );
    const storyScroll = container.querySelector<HTMLElement>(
      '[data-testid="landing-story-scroll"]',
    );

    expect(header?.className).toContain("h-[4.5rem]");
    expect(container.querySelector('[data-testid="landing-logo"]')).not.toBeNull();
    expect(brand?.className).toContain("text-2xl");
    expect(progress).toHaveLength(6);
    expect(intro?.getAttribute("aria-current")).toBe("step");
    expect(storyScroll).not.toBeNull();
    expect(storyScroll?.className).not.toContain("scroll-smooth");
    Object.defineProperty(storyScroll, "clientHeight", {
      configurable: true,
      value: 720,
    });

    await act(async () => {
      distributed?.click();
    });

    expect(mocks.scrollTo).toHaveBeenCalledWith({
      behavior: "auto",
      top: 3600,
    });
    expect(distributed?.getAttribute("aria-current")).toBe("step");
  });

  it("uses readable typography throughout the product story", async () => {
    const container = await render();
    const expectedScale = [
      ["landing-value-card-title", "text-lg"],
      ["landing-value-card-body", "text-base"],
      ["landing-story-body", "text-xl"],
      ["landing-proof-row", "text-lg"],
      ["landing-card-title", "text-lg"],
      ["landing-card-owner", "text-base"],
      ["landing-flow-node-title", "text-base"],
      ["landing-flow-node-owner", "text-sm"],
    ] as const;

    for (const [testId, className] of expectedScale) {
      const elements = container.querySelectorAll<HTMLElement>(
        `[data-testid="${testId}"]`,
      );
      expect(elements.length, testId).toBeGreaterThan(0);
      for (const element of elements) {
        expect(element.className, testId).toContain(className);
      }
    }

    for (const source of [messageGraphicSource, repositoryGraphicSource]) {
      const fontSizes = [...source.matchAll(/font-size="(\d+)"/g)].map(
        (match) => Number(match[1]),
      );
      expect(Math.min(...fontSizes)).toBeGreaterThanOrEqual(14);
    }
  });

  it("connect CTA enters existing Desktop Runtime setup flow", async () => {
    const container = await render();
    const connect = container.querySelector(
      '[data-testid="landing-cta-connect"]',
    ) as HTMLButtonElement | null;
    expect(connect).not.toBeNull();

    act(() => {
      connect?.click();
    });

    expect(mocks.setMode).toHaveBeenCalledWith("remote");
    expect(mocks.navigate).toHaveBeenCalledWith("/chat");
  });

  it("steps through the incident scenario when the user clicks next", async () => {
    installReducedMotion();
    const container = await render();
    const cta = container.querySelector(
      '[data-testid="landing-cta-demo"]',
    ) as HTMLButtonElement;
    await act(async () => {
      cta.click();
    });
    const next = container.querySelector(
      '[data-testid="demo-next"]',
    ) as HTMLButtonElement;
    expect(next).not.toBeNull();

    const cardIdx = incidentScenario.frames.findIndex(
      (f) => f.id === "teamup-card-two",
    );
    for (let i = 0; i <= cardIdx; i += 1) {
      await act(async () => {
        next.click();
      });
    }

    expect(
      container.querySelector('[data-testid="demo-card-wh-3a91"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="demo-card-wh-3a92"]'),
    ).not.toBeNull();
    expect(
      container.querySelector('[data-testid="demo-member-investigator"]'),
    ).not.toBeNull();

    // Continue to the end and verify the finale narration.
    const remaining = incidentScenario.frames.length - cardIdx - 1;
    for (let i = 0; i < remaining; i += 1) {
      await act(async () => {
        next.click();
      });
    }
    expect(container.textContent).toContain(
      "Two agents hired. Two cards closed. Twenty commits.",
    );
  });

  it("does not make network requests", async () => {
    const fetchSpy = vi
      .spyOn(globalThis, "fetch")
      .mockImplementation(() => Promise.resolve(new Response()));

    await render();

    expect(fetchSpy).not.toHaveBeenCalled();
    fetchSpy.mockRestore();
  });
});
