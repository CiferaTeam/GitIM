// @vitest-environment jsdom

import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Project } from "../../lib/types";
import { ChannelProjectMenu } from "./channel-project-controls";

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const projects: Project[] = [
  {
    slug: "design",
    meta: {
      display_name: "Design",
      created_by: "lewis",
      created_at: "2026-01-01T00:00:00Z",
      introduction: "Design work",
    },
    channel_count: 0,
  },
];

describe("ChannelProjectMenu", () => {
  let root: Root | null = null;
  let container: HTMLDivElement;

  afterEach(() => {
    act(() => root?.unmount());
    container?.remove();
  });

  it("keeps an open trigger visible and blocks options while busy", () => {
    const onAssign = vi.fn().mockResolvedValue(undefined);
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);

    act(() => {
      root?.render(
        <ChannelProjectMenu
          channel="general"
          projects={projects}
          currentProject={null}
          busy={false}
          onAssign={onAssign}
        />,
      );
    });
    const trigger = container.querySelector<HTMLButtonElement>(
      '[data-testid="channel-project-trigger-general"]',
    );
    act(() => trigger?.click());
    expect(trigger?.className).toContain("data-[state=open]:opacity-100");

    act(() => {
      root?.render(
        <ChannelProjectMenu
          channel="general"
          projects={projects}
          currentProject={null}
          busy
          onAssign={onAssign}
        />,
      );
    });
    const option = document.querySelector<HTMLButtonElement>(
      '[data-testid="channel-project-option-design"]',
    );
    expect(option?.disabled).toBe(true);
    act(() => option?.click());
    expect(onAssign).not.toHaveBeenCalled();
  });
});
