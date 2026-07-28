// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { LandingPage } from "./landing-page";
import { incidentScenario } from "@/lib/demo-story";

const mocks = vi.hoisted(() => ({
  setMode: vi.fn(),
  navigate: vi.fn(),
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

  it("opens text-first: demo stage mounts only after Watch the demo", async () => {
    const container = await render();
    expect(container.textContent).toContain("You shape the team");
    expect(container.textContent).toContain("Connect your runtime");
    expect(container.querySelector('[data-testid="demo-stage"]')).toBeNull();

    const cta = container.querySelector(
      '[data-testid="landing-cta-demo"]',
    ) as HTMLButtonElement | null;
    expect(cta).not.toBeNull();
    await act(async () => {
      cta?.click();
    });

    expect(
      container.querySelector('[data-testid="demo-stage"]'),
    ).not.toBeNull();
    // The CTA hides while the demo is open; the close button collapses it.
    expect(
      container.querySelector('[data-testid="landing-cta-demo"]'),
    ).toBeNull();
    const close = container.querySelector(
      '[data-testid="demo-close"]',
    ) as HTMLButtonElement | null;
    expect(close).not.toBeNull();
    await act(async () => {
      close?.click();
    });
    expect(container.querySelector('[data-testid="demo-stage"]')).toBeNull();
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
