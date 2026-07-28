// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { DemoStage } from "./demo-stage";
import { incidentScenario } from "@/lib/demo-story";

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

const frameIndex = (id: string) =>
  incidentScenario.frames.findIndex((f) => f.id === id);

describe("DemoStage", () => {
  let root: Root | null = null;
  let container: HTMLDivElement | null = null;

  beforeEach(() => {
    installReducedMotion();
  });

  afterEach(() => {
    if (root) {
      act(() => {
        root?.unmount();
      });
    }
    root = null;
    container = null;
    document.body.innerHTML = "";
  });

  async function render() {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root?.render(<DemoStage />);
    });
    return container;
  }

  async function stepTo(frameIdx: number) {
    const next = container?.querySelector(
      '[data-testid="demo-next"]',
    ) as HTMLButtonElement | null;
    expect(next).not.toBeNull();
    for (let i = 0; i <= frameIdx; i += 1) {
      await act(async () => {
        next?.click();
      });
    }
  }

  function anchorsForFrame(frameIdx: number): string[] {
    const frame = incidentScenario.frames[frameIdx];
    const ids = new Set<string>();
    for (const e of frame.effects ?? []) {
      if (e.kind === "arrow") {
        ids.add(e.from);
        ids.add(e.to);
      } else {
        ids.add(e.target);
      }
    }
    if (frame.typing) ids.add(frame.typing.anchor);
    return [...ids];
  }

  it("resolves every effect anchor in the DOM for all 28 frames (anchor contract)", async () => {
    for (let i = 0; i < incidentScenario.frames.length; i += 1) {
      const c = await render();
      await stepTo(i);
      for (const id of anchorsForFrame(i)) {
        expect(
          c.querySelector(`[data-anchor="${id}"]`),
          `frame "${incidentScenario.frames[i].id}" references missing anchor "${id}"`,
        ).not.toBeNull();
      }
      act(() => {
        root?.unmount();
      });
      root = null;
      container = null;
      document.body.innerHTML = "";
    }
  });

  it("shows the prefilled initial state before play (no empty box)", async () => {
    const c = await render();
    expect(c.querySelector('[data-anchor="chat-msg-1"]')).not.toBeNull();
    expect(c.querySelector('[data-anchor="chat-msg-2"]')).not.toBeNull();
    expect(c.querySelector('[data-anchor="member-lewis"]')).not.toBeNull();
    expect(c.querySelector('[data-anchor="member-coordinator"]')).not.toBeNull();
    expect(c.textContent).toContain("No cards yet.");
    expect(c.querySelector('[data-testid="demo-latest-commit"]')).not.toBeNull();
  });

  it("renders the narration bar with the current frame title and caption", async () => {
    const c = await render();
    await stepTo(0);
    expect(
      c.querySelector('[data-testid="demo-narration-title"]')?.textContent,
    ).toBe("The night before v2.4");
    expect(
      c.querySelector('[data-testid="demo-narration-caption"]')?.textContent,
    ).toContain("One human, one coordinator.");
  });

  it("renders the chapter progress bar with frame counter", async () => {
    const c = await render();
    expect(
      c.querySelector('[data-testid="demo-chapter-incident"]'),
    ).not.toBeNull();
    expect(c.querySelector('[data-testid="demo-chapter-teamup"]')).not.toBeNull();
    expect(
      c.querySelector('[data-testid="demo-chapter-delivery"]'),
    ).not.toBeNull();
    expect(
      c.querySelector('[data-testid="demo-frame-counter"]')?.textContent,
    ).toContain("0 / 28");
    await stepTo(0);
    expect(
      c.querySelector('[data-testid="demo-frame-counter"]')?.textContent,
    ).toContain("1 / 28");
  });

  it("jumps to a chapter's first frame when its segment is clicked", async () => {
    const c = await render();
    const teamup = c.querySelector(
      '[data-testid="demo-chapter-teamup"]',
    ) as HTMLButtonElement;
    await act(async () => {
      teamup.click();
    });
    expect(
      c.querySelector('[data-testid="demo-frame-counter"]')?.textContent,
    ).toContain("6 / 28");
    expect(
      c.querySelector('[data-testid="demo-narration-title"]')?.textContent,
    ).toBe("Intent becomes CLI calls");

    const delivery = c.querySelector(
      '[data-testid="demo-chapter-delivery"]',
    ) as HTMLButtonElement;
    await act(async () => {
      delivery.click();
    });
    expect(
      c.querySelector('[data-testid="demo-frame-counter"]')?.textContent,
    ).toContain("19 / 28");
    // Chapter 3 opens inside card wh-3a91's discussion view.
    expect(c.querySelector('[data-testid="demo-card-panel"]')).not.toBeNull();
    expect(c.textContent).toContain("back to #release-v2-4");
  });

  it("renders card discussion messages with visible body text in card view", async () => {
    const c = await render();
    await stepTo(frameIndex("delivery-handoff"));
    const panel = c.querySelector('[data-testid="demo-card-panel"]');
    expect(panel).not.toBeNull();
    // Both investigation messages render with their real body text.
    expect(panel?.textContent).toContain(
      "Found it. We ack before the dedupe check",
    );
    expect(panel?.textContent).toContain(
      "dedupe must run before ack, keyed on delivery id.",
    );
    expect(c.querySelector('[data-anchor="card-msg-1"]')).not.toBeNull();
    expect(c.querySelector('[data-anchor="card-msg-2"]')).not.toBeNull();
    // Card header breadcrumb shows the card id and title.
    expect(c.textContent).toContain("back to #release-v2-4");
    expect(c.textContent).toContain("Investigate duplicate webhook retries");
  });

  it("renders inline command chips under coordinator messages", async () => {
    const c = await render();
    await stepTo(frameIndex("teamup-second-command"));
    const chip = c.querySelector('[data-anchor="chat-msg-4-chip-2"]');
    expect(chip).not.toBeNull();
    expect(chip?.getAttribute("title")).toBe(
      "gitim-runtime add-agent --handler fixer --provider codex",
    );
    expect(chip?.textContent).toContain("$");
  });

  it("shows the highlight overlay on effect frames and clears it on the finale", async () => {
    const c = await render();
    await stepTo(frameIndex("incident-commit"));
    expect(
      c.querySelector('[data-testid="demo-effects-overlay"]'),
    ).not.toBeNull();

    await stepTo(frameIndex("delivery-finale"));
    expect(c.querySelector('[data-testid="demo-effects-overlay"]')).toBeNull();
    expect(
      c.querySelector('[data-testid="demo-narration-title"]')?.textContent,
    ).toBe("Audit complete");
    expect(
      c.querySelector('[data-testid="demo-narration-caption"]')?.textContent,
    ).toContain("Two agents hired. Two cards closed. Twenty commits.");
  });

  it("updates the latest commit row as frames apply", async () => {
    const c = await render();
    await stepTo(frameIndex("incident-commit"));
    expect(
      c.querySelector('[data-testid="demo-latest-commit"]')?.getAttribute(
        "title",
      ),
    ).toBe("msg: @lewis -> release-v2-4 L000003");
  });

  it("toggles narration mute from the controls bar", async () => {
    const c = await render();
    const mute = c.querySelector(
      '[data-testid="demo-mute"]',
    ) as HTMLButtonElement | null;
    expect(mute).not.toBeNull();
    expect(mute?.disabled).toBe(false);
    expect(mute?.getAttribute("aria-label")).toBe("Mute narration");

    await act(async () => {
      mute?.click();
    });
    expect(mute?.getAttribute("aria-label")).toBe("Unmute narration");
  });
});
