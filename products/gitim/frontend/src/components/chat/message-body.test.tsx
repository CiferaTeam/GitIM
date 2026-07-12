// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { MemoryRouter } from "react-router";
import { MessageBody } from "./message-body";
import { useCardStore } from "@/hooks/use-card-store";
import { useChatStore } from "@/hooks/use-chat-store";
import { formatAssetRef, type AssetRef } from "@/lib/asset-ref";
import type { Card } from "@/lib/types";

const { assetResolveUrlMock, connectionState, workspaceState } = vi.hoisted(() => ({
  assetResolveUrlMock: vi.fn(
    (slug: string, asset: AssetRef, options?: { download?: boolean }) =>
      `http://127.0.0.1:16868/workspaces/${slug}/assets/resolve/${asset.originRuntimeId}/${asset.sha256}?name=${encodeURIComponent(asset.name)}${options?.download ? "&download=1" : ""}`,
  ),
  connectionState: { mode: "remote" as "remote" | "local" },
  workspaceState: { activeSlug: "workspace" as string | null },
}));

vi.mock("../../lib/client", () => ({
  assetResolveUrl: assetResolveUrlMock,
}));

vi.mock("../../hooks/use-connection-store", () => ({
  useConnectionStore: (selector: (state: typeof connectionState) => unknown) =>
    selector(connectionState),
}));

vi.mock("../../hooks/use-workspace-store", () => ({
  useWorkspaceStore: (selector: (state: typeof workspaceState) => unknown) =>
    selector(workspaceState),
}));

vi.mock("./reference-preview", () => ({
  CardReferenceLink: ({
    reference,
  }: {
    reference: { cardId: string; line?: number };
  }) => (
    <span data-testid="card-ref">
      [{reference.cardId}:L{reference.line ?? ""}]
    </span>
  ),
  MessageReferenceLink: () => <span data-testid="message-ref" />,
}));

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

const card: Card = {
  card_id: "20260703-122930-bc0",
  channel: "gitim-pr47-sop-0702",
  title: "Recovery review 3",
  status: "done",
  labels: [],
  assignee: null,
  created_by: "cfo",
  created_at: "20260703T122930Z",
  updated_at: "20260703T123201Z",
};

const ORIGIN = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const HASH = "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88";

function assetRef({
  name = "fleet-assets.png",
  mediaType = "image/png",
  size = 184_203,
  width,
  height,
}: {
  name?: string;
  mediaType?: string;
  size?: number;
  width?: number;
  height?: number;
} = {}): string {
  return formatAssetRef({
    version: 1,
    originRuntimeId: ORIGIN,
    sha256: HASH,
    name,
    mediaType,
    size,
    ...(width !== undefined && height !== undefined && { width, height }),
  });
}

describe("MessageBody", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    assetResolveUrlMock.mockClear();
    useCardStore.getState().resetForWorkspaceSwitch();
    useCardStore.getState().upsertCard(card);
    useChatStore.setState({ currentChannel: "gitim-pr47-sop-0702" });
    connectionState.mode = "remote";
    workspaceState.activeSlug = "workspace";
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => {
      root.unmount();
    });
    container.remove();
  });

  it("folds trailing L-number text into the inline card reference", async () => {
    await act(async () => {
      root.render(
        <MemoryRouter>
          <MessageBody body="Recovery review (card `20260703-122930-bc0` L2)." />
        </MemoryRouter>,
      );
      await Promise.resolve();
    });

    expect(container.textContent).toContain("[20260703-122930-bc0:L2]");
    expect(container.textContent).not.toContain(" L2).");
  });

  async function renderBody(body: string) {
    await act(async () => {
      root.render(
        <MemoryRouter>
          <MessageBody body={body} />
        </MemoryRouter>,
      );
      await Promise.resolve();
    });
  }

  it("renders a canonical PNG as a lazy anonymous image linked to the local resolver", async () => {
    await renderBody(assetRef({ width: 1600, height: 900 }));

    const image = container.querySelector("img[data-asset-image]");
    expect(image).toBeInstanceOf(HTMLImageElement);
    expect(image?.getAttribute("alt")).toBe("fleet-assets.png");
    expect(image?.getAttribute("loading")).toBe("lazy");
    expect(image?.getAttribute("crossorigin")).toBe("anonymous");
    expect(image?.getAttribute("src")).toContain("/assets/resolve/");

    const link = image?.closest("a");
    expect(link?.getAttribute("target")).toBe("_blank");
    expect(link?.getAttribute("rel")).toBe("noopener noreferrer");
    expect(assetResolveUrlMock).toHaveBeenCalledWith(
      "workspace",
      expect.objectContaining({ name: "fleet-assets.png", mediaType: "image/png" }),
    );
  });

  it("reserves exact normal image geometry and bounds extreme dimension hints", async () => {
    const normal = assetRef({ width: 1600, height: 900 });
    const extreme = assetRef({
      name: "extreme.png",
      width: 0xffff_ffff,
      height: 1,
    });
    await renderBody(`${normal}\n${extreme}`);

    const frames = Array.from(
      container.querySelectorAll<HTMLElement>("[data-asset-frame]"),
    );
    const images = Array.from(
      container.querySelectorAll<HTMLImageElement>("img[data-asset-image]"),
    );
    expect(frames).toHaveLength(2);
    expect(images).toHaveLength(2);
    expect(frames[0].style.aspectRatio).toBe("1600 / 900");
    expect(images[0].getAttribute("width")).toBe("1600");
    expect(images[0].getAttribute("height")).toBe("900");
    expect(frames[1].style.aspectRatio).toBe("4 / 1");
    expect(images[1].getAttribute("width")).not.toBe(String(0xffff_ffff));
    expect(frames[1].className).toContain("max-h-[440px]");
    expect(frames[1].className).toContain("min-h-");
  });

  it("preserves the clamped portrait ratio within the frame bounds", async () => {
    await renderBody(assetRef({
      name: "portrait.png",
      width: 1,
      height: 0xffff_ffff,
    }));

    const frame = container.querySelector<HTMLElement>("[data-asset-frame]")!;
    expect(frame.style.aspectRatio).toBe("1 / 4");
    expect(frame.style.width).toBe("100%");
    expect(frame.style.maxWidth).toBe("110px");
    expect(frame.className).toContain("max-h-[440px]");
    expect(frame.className).not.toContain("w-full");
  });

  it("moves from loading to loaded and supports stable repeated image retries", async () => {
    await renderBody(assetRef({ width: 800, height: 600 }));

    const firstImage = container.querySelector<HTMLImageElement>("img[data-asset-image]")!;
    const immutableUrl = firstImage.src;
    expect(container.querySelector('[role="status"]')?.textContent).toContain("Loading");

    act(() => firstImage.dispatchEvent(new Event("load", { bubbles: false })));
    expect(container.querySelector('[role="status"]')).toBeNull();

    act(() => firstImage.dispatchEvent(new Event("error", { bubbles: false })));
    expect(container.querySelector("img[data-asset-image]")).toBeNull();
    expect(container.textContent).toContain("Unavailable");
    expect(container.textContent).toContain("fleet-assets.png");

    const firstRetry = container.querySelector<HTMLButtonElement>(
      'button[aria-label="Retry loading fleet-assets.png"]',
    )!;
    act(() => firstRetry.click());
    const secondImage = container.querySelector<HTMLImageElement>("img[data-asset-image]")!;
    expect(secondImage).not.toBe(firstImage);
    expect(secondImage.src).toBe(immutableUrl);
    expect(container.querySelector('[role="status"]')?.textContent).toContain("Loading");

    act(() => secondImage.dispatchEvent(new Event("error", { bubbles: false })));
    expect(container.querySelectorAll("img[data-asset-image]")).toHaveLength(0);
    expect(
      container.querySelectorAll('button[aria-label="Retry loading fleet-assets.png"]'),
    ).toHaveLength(1);
  });

  it("renders PDF and unknown media as downloadable metadata cards", async () => {
    const pdf = assetRef({
      name: "protocol.pdf",
      mediaType: "application/pdf",
      size: 2 * 1024 * 1024,
    });
    const unknown = assetRef({
      name: "archive.bin",
      mediaType: "application/octet-stream",
      size: 1536,
    });
    const empty = assetRef({
      name: "empty.txt",
      mediaType: "text/plain",
      size: 0,
    });
    await renderBody(`${pdf}\n${unknown}\n${empty}`);

    expect(container.querySelector("img")).toBeNull();
    expect(container.textContent).toContain("protocol.pdf");
    expect(container.textContent).toContain("application/pdf");
    expect(container.textContent).toContain("2.0 MiB");
    expect(container.textContent).toContain("archive.bin");
    expect(container.textContent).toContain("application/octet-stream");
    expect(container.textContent).toContain("1.5 KiB");
    expect(container.textContent).toContain("0 B");
    expect(container.textContent).toContain("Origin 3c6a295e…");
    expect(container.querySelector(`[title="${ORIGIN}"]`)).not.toBeNull();

    const downloads = Array.from(
      container.querySelectorAll<HTMLAnchorElement>('a[aria-label^="Download "]'),
    );
    expect(downloads).toHaveLength(3);
    for (const download of downloads) {
      expect(download.href).toContain("download=1");
      expect(download.getAttribute("target")).toBe("_blank");
      expect(download.getAttribute("rel")).toBe("noopener noreferrer");
    }
  });

  it.each([
    ["image/svg+xml", "vector.svg"],
    ["text/html", "page.html"],
  ])("keeps %s on the file-card path", async (mediaType, name) => {
    await renderBody(assetRef({ name, mediaType }));

    expect(container.querySelector("img")).toBeNull();
    expect(container.querySelector(`a[aria-label="Download ${name}"]`)).not.toBeNull();
  });

  it.each([
    ["local", "browser-workspace"],
    ["remote", null],
  ] as const)(
    "renders Runtime-required metadata without resolving a URL in %s mode with workspace %s",
    async (mode, activeSlug) => {
      connectionState.mode = mode;
      workspaceState.activeSlug = activeSlug;
      await renderBody(assetRef());

      expect(container.textContent).toContain("Runtime required");
      expect(container.querySelector("img")).toBeNull();
      const disabled = container.querySelector<HTMLButtonElement>(
        'button[aria-label="Download fleet-assets.png"]',
      );
      expect(disabled?.disabled).toBe(true);
      expect(container.querySelector('a[aria-label="Download fleet-assets.png"]')).toBeNull();
      expect(assetResolveUrlMock).not.toHaveBeenCalled();
    },
  );

  it("renders Unicode and markup-like filenames as text with full accessible labels", async () => {
    const name = "报告 <b>状态.png";
    await renderBody(assetRef({ name, width: 640, height: 480 }));

    const image = container.querySelector("img[data-asset-image]");
    expect(image?.getAttribute("alt")).toBe(name);
    expect(container.querySelector(`[title="${name}"]`)?.textContent).toBe(name);
    expect(container.querySelector(`a[aria-label="Open ${name}"]`)).not.toBeNull();
    expect(container.querySelector("b")).toBeNull();
  });

  it("preserves invalid noncanonical references as exact selectable plain text", async () => {
    const invalid = assetRef().replace("size=184203", "size=0184203");
    await renderBody(invalid);

    expect(container.textContent).toBe(invalid);
    expect(container.querySelector("[data-asset-root]")).toBeNull();
  });

  it("preserves mixed text whitespace and asset ordering", async () => {
    const image = assetRef({ name: "one.png", width: 640, height: 480 });
    const file = assetRef({ name: "two.pdf", mediaType: "application/pdf" });
    await renderBody(`before ${image} middle\n${file} after`);

    const body = container.querySelector<HTMLElement>(".whitespace-pre-wrap")!;
    const children = Array.from(body.children);
    expect(children).toHaveLength(5);
    expect(children[0].textContent).toBe("before ");
    expect(children[1].hasAttribute("data-asset-root")).toBe(true);
    expect(children[2].textContent).toBe(" middle\n");
    expect(children[3].hasAttribute("data-asset-root")).toBe(true);
    expect(children[4].textContent).toBe(" after");
    expect(container.querySelectorAll("[data-asset-root]")).toHaveLength(2);
  });

  it("keeps inline and fenced asset references as code", async () => {
    const ref = assetRef();
    await renderBody(`\`${ref}\`\n\`\`\`text\n${ref}\n\`\`\``);

    expect(container.querySelector("[data-asset-root]")).toBeNull();
    expect(container.querySelectorAll("code")).toHaveLength(2);
    expect(container.textContent).toContain(ref);
  });

  it("stops asset click boundaries without preventing action defaults", async () => {
    const onClick = vi.fn();
    const onDoubleClick = vi.fn();
    const image = assetRef({ width: 640, height: 480 });
    const file = assetRef({ name: "notes.pdf", mediaType: "application/pdf" });
    await act(async () => {
      root.render(
        <MemoryRouter>
          <span onClick={onClick} onDoubleClick={onDoubleClick}>
            <MessageBody body={`${image}\n${file}`} />
          </span>
        </MemoryRouter>,
      );
      await Promise.resolve();
    });

    const imageLink = container.querySelector<HTMLAnchorElement>('a[aria-label^="Open "]')!;
    const download = container.querySelector<HTMLAnchorElement>('a[aria-label^="Download "]')!;
    for (const target of [
      container.querySelector<HTMLElement>("[data-asset-root]")!,
      imageLink,
      download,
    ]) {
      const click = new MouseEvent("click", { bubbles: true, cancelable: true });
      const doubleClick = new MouseEvent("dblclick", { bubbles: true, cancelable: true });
      act(() => {
        target.dispatchEvent(click);
        target.dispatchEvent(doubleClick);
      });
      expect(click.defaultPrevented).toBe(false);
      expect(doubleClick.defaultPrevented).toBe(false);
    }

    const liveImage = container.querySelector<HTMLImageElement>("img[data-asset-image]")!;
    act(() => liveImage.dispatchEvent(new Event("error")));
    const retry = container.querySelector<HTMLButtonElement>('button[aria-label^="Retry loading "]')!;
    const retryClick = new MouseEvent("click", { bubbles: true, cancelable: true });
    const retryDoubleClick = new MouseEvent("dblclick", { bubbles: true, cancelable: true });
    act(() => {
      retry.dispatchEvent(retryDoubleClick);
      retry.dispatchEvent(retryClick);
    });
    expect(retryClick.defaultPrevented).toBe(false);
    expect(retryDoubleClick.defaultPrevented).toBe(false);
    expect(onClick).not.toHaveBeenCalled();
    expect(onDoubleClick).not.toHaveBeenCalled();
  });

  it("keeps existing inline fragment renderers working together", async () => {
    await renderBody(
      "<@alice> <#general> <#dev:L000042> <#general/abc123> <~bob> <!https://example.com|Example> `code` **bold** *italic*",
    );

    expect(container.textContent).toContain("@alice");
    expect(container.textContent).toContain("#general");
    expect(container.querySelector('[data-testid="message-ref"]')).not.toBeNull();
    expect(container.querySelector('[data-testid="card-ref"]')).not.toBeNull();
    expect(container.textContent).toContain("~bob");
    expect(container.querySelector('a[href="https://example.com"]')?.textContent).toBe("Example");
    expect(container.querySelector("code")?.textContent).toBe("code");
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    expect(container.querySelector("em")?.textContent).toBe("italic");
  });
});
