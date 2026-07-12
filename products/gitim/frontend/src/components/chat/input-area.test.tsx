// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";
import { formatAssetRef } from "../../lib/asset-ref";
import type { UploadedAsset } from "../../lib/client";
import type { Channel, Message } from "../../lib/types";
import {
  attachmentDraftKey,
  useAttachmentDraftStore,
} from "../../hooks/use-attachment-draft-store";
import { InputArea } from "./input-area";

const { uploadAssetsMock, mediaState, connectionState } = vi.hoisted(() => ({
  uploadAssetsMock: vi.fn(),
  mediaState: { mobile: false },
  connectionState: { mode: "remote" as "remote" | "local" },
}));

vi.mock("../../lib/client", () => {
  return { uploadAssets: uploadAssetsMock };
});

vi.mock("../../hooks/use-connection-store", () => ({
  useConnectionStore: (selector: (state: typeof connectionState) => unknown) =>
    selector(connectionState),
}));

vi.mock("../../hooks/use-media-query", () => ({
  useIsMobile: () => mediaState.mobile,
}));

const memoryStorage = (() => {
  const m = new Map<string, string>();
  return {
    get length() {
      return m.size;
    },
    clear: () => m.clear(),
    getItem: (k: string) => m.get(k) ?? null,
    key: (i: number) => Array.from(m.keys())[i] ?? null,
    removeItem: (k: string) => m.delete(k),
    setItem: (k: string, v: string) => {
      m.set(k, v);
    },
  } as Storage;
})();
Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: memoryStorage,
});

Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });

function setTextareaValue(textarea: HTMLTextAreaElement, value: string) {
  const valueSetter = Object.getOwnPropertyDescriptor(
    HTMLTextAreaElement.prototype,
    "value",
  )?.set;
  valueSetter?.call(textarea, value);
  textarea.dispatchEvent(new Event("input", { bubbles: true }));
}

const noopSend = vi.fn(async () => ({ ok: true as const }));

function uploadedAsset(file: File): UploadedAsset {
  const base = {
    version: 1 as const,
    originRuntimeId: "12345678-1234-1234-1234-123456789abc",
    sha256: "a".repeat(64),
    name: file.name,
    mediaType: file.type || "application/octet-stream",
    size: file.size,
  };
  const ref = formatAssetRef(base);
  return { ...base, raw: ref, ref };
}

function pasteFiles(target: HTMLTextAreaElement, files: File[]) {
  const event = new Event("paste", { bubbles: true, cancelable: true });
  Object.defineProperty(event, "clipboardData", {
    value: { files },
  });
  target.dispatchEvent(event);
  return event;
}

function selectFiles(input: HTMLInputElement, files: File[], fakeValue?: string) {
  Object.defineProperty(input, "files", { configurable: true, value: files });
  if (fakeValue !== undefined) {
    Object.defineProperty(input, "value", {
      configurable: true,
      writable: true,
      value: fakeValue,
    });
  }
  input.dispatchEvent(new Event("change", { bubbles: true }));
}

function pressEnter(textarea: HTMLTextAreaElement) {
  textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

const channel: Channel = {
  name: "general",
  kind: "channel",
  unreadCount: 0,
  hasMention: false,
  members: ["owner"],
  created_by: "owner",
};

const defaultInputProps = {
  workspaceSlug: "workspace",
  workspaceKey: "ws",
  scopeKey: "general",
  replyTo: null as Message | null,
  onReplyToChange: () => {},
  mentionCandidates: [] as string[],
  routing: { kind: "channel" as const, channel },
  messages: [] as Message[],
  currentUser: "me",
  onSend: noopSend,
};

describe("InputArea card recipient preview", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    container.remove();
    useAttachmentDraftStore.getState().disposeAll();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("previews the reporter and assignee for a card draft", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(
        <InputArea
          workspaceSlug="workspace"
          workspaceKey="ws"
          scopeKey="card:strategy/20260616-174312-42b"
          replyTo={null}
          onReplyToChange={() => {}}
          mentionCandidates={[]}
          routing={{ kind: "card", card: { created_by: "leader1", assignee: "leader2" } }}
          currentUser="lewis"
          onSend={noopSend}
        />,
      );
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    expect(textarea).not.toBeNull();

    await act(async () => {
      setTextareaValue(textarea!, "ss");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview).not.toBeNull();
    expect(preview?.textContent).toContain("@leader1");
    expect(preview?.textContent).toContain("@leader2");
    expect(preview?.textContent).not.toContain("no one else");
  });

  it("excludes the current user when they are a card role", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(
        <InputArea
          workspaceSlug="workspace"
          workspaceKey="ws"
          scopeKey="card:strategy/c1"
          replyTo={null}
          onReplyToChange={() => {}}
          mentionCandidates={[]}
          routing={{ kind: "card", card: { created_by: "leader1", assignee: "lewis" } }}
          currentUser="lewis"
          onSend={noopSend}
        />,
      );
      await Promise.resolve();
    });

    const textarea = document.querySelector("textarea");
    await act(async () => {
      setTextareaValue(textarea!, "ss");
      await Promise.resolve();
    });

    const preview = document.querySelector("[data-recipient-preview]");
    expect(preview?.textContent).toContain("@leader1");
    expect(preview?.textContent).not.toContain("@lewis");
  });
});

describe("InputArea attachments", () => {
  let container: HTMLDivElement;
  let root: Root | null = null;

  async function renderInput(node: ReactNode) {
    await act(async () => {
      root = createRoot(container);
      root.render(node);
      await Promise.resolve();
    });
  }

  beforeEach(() => {
    uploadAssetsMock.mockReset();
    noopSend.mockClear();
    mediaState.mobile = false;
    memoryStorage.clear();
    useAttachmentDraftStore.getState().disposeAll();
    connectionState.mode = "remote";
    container = document.createElement("div");
    document.body.appendChild(container);
  });

  afterEach(() => {
    act(() => root?.unmount());
    root = null;
    container.remove();
    useAttachmentDraftStore.getState().disposeAll();
    vi.unstubAllGlobals();
    vi.restoreAllMocks();
  });

  it("pastes an image and sends an attachment-only message", async () => {
    const file = new File(["image"], "diagram.png", { type: "image/png" });
    const asset = uploadedAsset(file);
    const onSend = vi.fn(async () => ({ ok: true as const }));
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });

    await act(async () => {
      root = createRoot(container);
      root.render(
        <InputArea
          {...defaultInputProps}
          onSend={onSend}
        />,
      );
      await Promise.resolve();
    });

    const textarea = container.querySelector("textarea")!;
    let pasteEvent!: Event;
    await act(async () => {
      pasteEvent = pasteFiles(textarea, [file]);
      await Promise.resolve();
    });

    expect(pasteEvent.defaultPrevented).toBe(true);
    expect(container.querySelector("[data-attachment-draft-strip]")?.textContent)
      .toContain("diagram.png");
    expect(container.querySelector("[data-recipient-preview]")?.textContent)
      .toContain("@owner");

    await act(async () => {
      textarea.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(uploadAssetsMock).toHaveBeenCalledWith("workspace", [file]);
    expect(onSend).toHaveBeenCalledWith(asset.ref, 0);
  });

  it("preserves picker order, resets the input, and allows remove then reselect", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(<InputArea {...defaultInputProps} />);
      await Promise.resolve();
    });

    const picker = container.querySelector<HTMLInputElement>('input[type="file"]')!;
    const first = new File(["a"], "first.txt", { type: "text/plain" });
    const second = new File(["bb"], "second.pdf", { type: "application/pdf" });
    await act(async () => {
      selectFiles(picker, [first, second], "C:\\fakepath\\second.pdf");
      await Promise.resolve();
    });

    expect(picker.value).toBe("");
    const strip = container.querySelector("[data-attachment-draft-strip]")!;
    expect(strip.textContent?.indexOf("first.txt")).toBeLessThan(
      strip.textContent?.indexOf("second.pdf") ?? -1,
    );

    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Remove first.txt"]')!.click();
      await Promise.resolve();
      selectFiles(picker, [first]);
      await Promise.resolve();
    });
    expect(container.querySelector('button[aria-label="Remove first.txt"]')).not.toBeNull();
  });

  it("keeps text-only paste native and prevents file paste placeholders", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(<InputArea {...defaultInputProps} />);
      await Promise.resolve();
    });
    const textarea = container.querySelector("textarea")!;

    let textEvent!: Event;
    let fileEvent!: Event;
    await act(async () => {
      textEvent = pasteFiles(textarea, []);
      fileEvent = pasteFiles(textarea, [new File(["x"], "x.txt")]);
      await Promise.resolve();
    });
    expect(textEvent.defaultPrevented).toBe(false);
    expect(fileEvent.defaultPrevented).toBe(true);
  });

  it("accepts valid files beside invalid files and enforces the ten-item limit", async () => {
    await act(async () => {
      root = createRoot(container);
      root.render(<InputArea {...defaultInputProps} />);
      await Promise.resolve();
    });
    const textarea = container.querySelector("textarea")!;
    const invalid = new File(["x"], "too-large.bin");
    Object.defineProperty(invalid, "size", { value: 50 * 1024 * 1024 + 1 });
    const valid = new File(["ok"], "valid.txt");

    await act(async () => {
      pasteFiles(textarea, [valid, invalid]);
      await Promise.resolve();
    });
    expect(container.textContent).toContain("valid.txt");
    expect(container.textContent).toContain("50 MiB");

    const picker = container.querySelector<HTMLInputElement>('input[type="file"]')!;
    const remaining = Array.from({ length: 10 }, (_, index) =>
      new File([String(index)], `file-${index}.txt`));
    await act(async () => {
      selectFiles(picker, remaining);
      await Promise.resolve();
    });
    expect(container.querySelectorAll('[aria-label^="Remove "]')).toHaveLength(10);
    expect(container.textContent).toContain("at most 10 attachments");
  });

  it("reuses uploaded refs after send failure and uploads only newly added files", async () => {
    const first = new File(["a"], "first.txt", { type: "text/plain" });
    const second = new File(["bb"], "second.txt", { type: "text/plain" });
    const firstAsset = uploadedAsset(first);
    const secondAsset = { ...uploadedAsset(second), sha256: "b".repeat(64) };
    const secondRef = formatAssetRef({
      version: secondAsset.version,
      originRuntimeId: secondAsset.originRuntimeId,
      sha256: secondAsset.sha256,
      name: secondAsset.name,
      mediaType: secondAsset.mediaType,
      size: secondAsset.size,
    });
    secondAsset.raw = secondRef;
    secondAsset.ref = secondRef;
    const onSend = vi.fn()
      .mockResolvedValueOnce({ ok: false, error: "send failed" })
      .mockResolvedValueOnce({ ok: true });
    uploadAssetsMock
      .mockResolvedValueOnce({ ok: true, data: { assets: [firstAsset] } })
      .mockResolvedValueOnce({ ok: true, data: { assets: [secondAsset] } });

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [first]);
      await Promise.resolve();
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("send failed");

    await act(async () => {
      selectFiles(container.querySelector('input[type="file"]')!, [second]);
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(uploadAssetsMock).toHaveBeenCalledTimes(2);
    expect(uploadAssetsMock.mock.calls[0][1]).toEqual([first]);
    expect(uploadAssetsMock.mock.calls[1][1]).toEqual([second]);
    expect(onSend).toHaveBeenCalledTimes(2);
    expect(onSend.mock.calls[1]).toEqual([`${firstAsset.ref}\n${secondRef}`, 0]);
  });

  it("retries a failed send without uploading existing refs again", async () => {
    const file = new File(["retry"], "retry.txt", { type: "text/plain" });
    const asset = uploadedAsset(file);
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });
    const onSend = vi.fn()
      .mockResolvedValueOnce({ ok: false, error: "send failed" })
      .mockResolvedValueOnce({ ok: true });

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(uploadAssetsMock).toHaveBeenCalledTimes(1);
    expect(onSend).toHaveBeenCalledTimes(2);
    expect(onSend).toHaveBeenLastCalledWith(asset.ref, 0);
  });

  it("composes trimmed text followed by refs in selection order", async () => {
    const files = [new File(["1"], "one.txt"), new File(["22"], "two.txt")];
    const assets = files.map(uploadedAsset);
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets } });
    const onSend = vi.fn(async () => ({ ok: true as const }));

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      selectFiles(container.querySelector('input[type="file"]')!, files);
      setTextareaValue(container.querySelector("textarea")!, "  context  ");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith(`context\n${assets[0].ref}\n${assets[1].ref}`, 0);
  });

  it.each([
    ["channel", { kind: "channel" as const, channel }, "@owner"],
    [
      "dm",
      {
        kind: "channel" as const,
        channel: { ...channel, name: "dm", kind: "dm" as const, members: ["me", "peer"] },
      },
      "@peer",
    ],
    [
      "card",
      { kind: "card" as const, card: { created_by: "reporter", assignee: "assignee" } },
      "@reporter",
    ],
  ])("previews %s recipients for attachment-only drafts", async (_name, routing, expected) => {
    await renderInput(<InputArea {...defaultInputProps} routing={routing} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [new File(["x"], `${_name}.txt`)]);
      await Promise.resolve();
    });
    expect(container.querySelector("[data-recipient-preview]")?.textContent).toContain(expected);
  });

  it("confirms empty attachment routing without sending the internal sentinel", async () => {
    const file = new File(["x"], "solo.txt");
    const asset = uploadedAsset(file);
    const onSend = vi.fn(async () => ({ ok: true as const }));
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });

    await renderInput(
      <InputArea
        {...defaultInputProps}
        routing={{ kind: "channel", channel: { ...channel, created_by: "me" } }}
        onSend={onSend}
      />,
    );
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });
    expect(document.querySelector('[data-testid="empty-recipients-dialog"]')).not.toBeNull();
    await act(async () => {
      document.querySelector<HTMLButtonElement>('[data-testid="empty-recipients-confirm"]')!.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith(asset.ref, 0);
  });

  it("preserves reply and text when an attachment send fails", async () => {
    const file = new File(["x"], "reply.txt");
    const asset = uploadedAsset(file);
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });
    const reply = { author: "owner", body: "parent", line_number: 42, point_to: 0 } as Message;
    const onReplyToChange = vi.fn();
    const onSend = vi.fn(async () => ({ ok: false, error: "offline" }));

    await renderInput(
      <InputArea
        {...defaultInputProps}
        replyTo={reply}
        onReplyToChange={onReplyToChange}
        onSend={onSend}
      />,
    );
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      setTextareaValue(container.querySelector("textarea")!, " keep me ");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith(`keep me\n${asset.ref}`, 42);
    expect(onReplyToChange).not.toHaveBeenCalled();
    expect(container.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe(" keep me ");
    expect(localStorage.getItem("gitim:draft:ws:general")).toBe(" keep me ");
    expect(container.textContent).toContain("Reply to");
    expect(container.textContent).toContain("offline");
  });

  it("finishes a captured attachment send after unmount without mutating component state", async () => {
    const file = new File(["asset"], "unmount.txt", { type: "text/plain" });
    const asset = uploadedAsset(file);
    const send = deferred<{ ok: true }>();
    const onSend = vi.fn(() => send.promise);
    const onReplyToChange = vi.fn();
    const focus = vi.spyOn(HTMLTextAreaElement.prototype, "focus");
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });

    await renderInput(
      <InputArea
        {...defaultInputProps}
        onReplyToChange={onReplyToChange}
        onSend={onSend}
      />,
    );
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      setTextareaValue(container.querySelector("textarea")!, "caption");
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith(`caption\n${asset.ref}`, 0);
    focus.mockClear();
    consoleError.mockClear();

    await act(async () => {
      root?.unmount();
      root = null;
      await Promise.resolve();
    });
    await act(async () => {
      send.resolve({ ok: true });
      await Promise.resolve();
      await Promise.resolve();
    });

    const key = attachmentDraftKey("ws", "general");
    expect(useAttachmentDraftStore.getState().drafts[key]).toBeUndefined();
    expect(localStorage.getItem("gitim:draft:ws:general")).toBeNull();
    expect(onReplyToChange).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
    expect(consoleError).not.toHaveBeenCalled();
  });

  it("finishes a captured text send after unmount without mutating component state", async () => {
    const send = deferred<{ ok: true }>();
    const onSend = vi.fn(() => send.promise);
    const onReplyToChange = vi.fn();
    const focus = vi.spyOn(HTMLTextAreaElement.prototype, "focus");
    const consoleError = vi.spyOn(console, "error").mockImplementation(() => {});

    await renderInput(
      <InputArea
        {...defaultInputProps}
        onReplyToChange={onReplyToChange}
        onSend={onSend}
      />,
    );
    await act(async () => {
      setTextareaValue(container.querySelector("textarea")!, "text survives");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith("text survives", 0);
    focus.mockClear();
    consoleError.mockClear();

    await act(async () => {
      root?.unmount();
      root = null;
      await Promise.resolve();
    });
    await act(async () => {
      send.resolve({ ok: true });
      await Promise.resolve();
      await Promise.resolve();
    });

    expect(localStorage.getItem("gitim:draft:ws:general")).toBeNull();
    expect(onReplyToChange).not.toHaveBeenCalled();
    expect(focus).not.toHaveBeenCalled();
    expect(consoleError).not.toHaveBeenCalled();
  });

  it("replaces a text send error with one attachment validation error", async () => {
    const onSend = vi.fn(async () => ({ ok: false, error: "text send failed" }));
    const valid = new File(["valid"], "valid.txt", { type: "text/plain" });
    const invalid = new File(["large"], "large.bin");
    Object.defineProperty(invalid, "size", { value: 50 * 1024 * 1024 + 1 });

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      setTextareaValue(container.querySelector("textarea")!, "keep this text");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(container.textContent).toContain("text send failed");

    await act(async () => {
      selectFiles(container.querySelector('input[type="file"]')!, [valid, invalid]);
      await Promise.resolve();
    });

    expect(container.textContent).not.toContain("text send failed");
    const errors = container.querySelectorAll('[role="alert"]');
    expect(errors).toHaveLength(1);
    expect(errors[0].textContent).toContain("50 MiB");
    expect(container.textContent).toContain("valid.txt");
    expect(container.querySelector<HTMLTextAreaElement>("textarea")?.value)
      .toBe("keep this text");
  });

  it("announces a text-only send error accessibly", async () => {
    const onSend = vi.fn(async () => ({ ok: false, error: "text send failed" }));
    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      setTextareaValue(container.querySelector("textarea")!, "keep this text");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(container.querySelector('[role="alert"]')?.textContent)
      .toContain("text send failed");
  });

  it("captures the old scope while a deferred upload leaves the new scope usable", async () => {
    const oldFile = new File(["old"], "old.txt");
    const newFile = new File(["new"], "new.txt");
    const oldAsset = uploadedAsset(oldFile);
    const upload = deferred<{ ok: true; data: { assets: UploadedAsset[] } }>();
    uploadAssetsMock.mockReturnValueOnce(upload.promise);
    const oldSend = vi.fn(async () => ({ ok: true as const }));
    const newSend = vi.fn(async () => ({ ok: true as const }));
    const oldReply = { author: "owner", body: "old parent", line_number: 9, point_to: 0 } as Message;

    await renderInput(
      <InputArea {...defaultInputProps} replyTo={oldReply} onSend={oldSend} />,
    );
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [oldFile]);
      setTextareaValue(container.querySelector("textarea")!, "old text");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });

    await act(async () => {
      root!.render(
        <InputArea {...defaultInputProps} scopeKey="other" replyTo={null} onSend={newSend} />,
      );
      await Promise.resolve();
    });
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [newFile]);
      setTextareaValue(container.querySelector("textarea")!, "new text");
      await Promise.resolve();
    });
    expect(container.querySelector<HTMLTextAreaElement>("textarea")?.disabled).toBe(false);
    expect(container.textContent).toContain("new.txt");

    await act(async () => {
      upload.resolve({ ok: true, data: { assets: [oldAsset] } });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(oldSend).toHaveBeenCalledWith(`old text\n${oldAsset.ref}`, 9);
    expect(newSend).not.toHaveBeenCalled();
    expect(container.querySelector<HTMLTextAreaElement>("textarea")?.value).toBe("new text");
    expect(container.textContent).toContain("new.txt");
  });

  it("ignores stale completion after reset and recreation at the same key", async () => {
    let preview = 0;
    const revoke = vi.fn();
    vi.stubGlobal("URL", {
      ...URL,
      createObjectURL: vi.fn(() => `blob:preview-${++preview}`),
      revokeObjectURL: revoke,
    });
    const oldFile = new File(["old"], "old.png", { type: "image/png" });
    const newFile = new File(["new"], "new.png", { type: "image/png" });
    const upload = deferred<{ ok: true; data: { assets: UploadedAsset[] } }>();
    uploadAssetsMock.mockReturnValue(upload.promise);
    const onSend = vi.fn(async () => ({ ok: true as const }));

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [oldFile]);
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });
    expect(uploadAssetsMock).toHaveBeenCalledTimes(1);
    await act(async () => {
      const key = JSON.stringify(["ws", "general"]);
      useAttachmentDraftStore.getState().resetDraft(key);
      useAttachmentDraftStore.getState().addFiles(key, [newFile]);
      await Promise.resolve();
    });
    await act(async () => {
      upload.resolve({ ok: true, data: { assets: [uploadedAsset(oldFile)] } });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).not.toHaveBeenCalled();
    expect(container.textContent).toContain("new.png");
    expect(revoke).not.toHaveBeenCalledWith("blob:preview-2");
  });

  it("disables attachments in browser mode while preserving text send", async () => {
    connectionState.mode = "local";
    const onSend = vi.fn(async () => ({ ok: true as const }));
    await act(async () => {
      root = createRoot(container);
      root.render(<InputArea {...defaultInputProps} onSend={onSend} />);
      await Promise.resolve();
    });
    const attach = container.querySelector<HTMLButtonElement>('button[aria-label*="Runtime"]')!;
    expect(attach.disabled).toBe(true);
    await act(async () => {
      setTextareaValue(container.querySelector("textarea")!, "browser text");
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith("browser text", 0);
    expect(uploadAssetsMock).not.toHaveBeenCalled();
  });

  it("disables removal while uploading and revokes previews after success", async () => {
    const createObjectURL = vi.fn(() => "blob:preview");
    const revokeObjectURL = vi.fn();
    vi.stubGlobal("URL", { ...URL, createObjectURL, revokeObjectURL });
    const file = new File(["image"], "busy.png", { type: "image/png" });
    const asset = uploadedAsset(file);
    const upload = deferred<{ ok: true; data: { assets: UploadedAsset[] } }>();
    uploadAssetsMock.mockReturnValue(upload.promise);

    await renderInput(<InputArea {...defaultInputProps} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      await Promise.resolve();
    });
    await act(async () => {
      pressEnter(container.querySelector("textarea")!);
      await Promise.resolve();
    });
    expect(container.textContent).toContain("Uploading…");
    expect(container.querySelector<HTMLButtonElement>('button[aria-label="Remove busy.png"]')?.disabled)
      .toBe(true);
    expect(container.querySelector('img[src="blob:preview"]')).not.toBeNull();

    await act(async () => {
      upload.resolve({ ok: true, data: { assets: [asset] } });
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:preview");
  });

  it("supports the mobile send button for attachment-only drafts", async () => {
    mediaState.mobile = true;
    const file = new File(["x"], "mobile.txt");
    const asset = uploadedAsset(file);
    uploadAssetsMock.mockResolvedValue({ ok: true, data: { assets: [asset] } });
    const onSend = vi.fn(async () => ({ ok: true as const }));

    await renderInput(<InputArea {...defaultInputProps} onSend={onSend} />);
    await act(async () => {
      pasteFiles(container.querySelector("textarea")!, [file]);
      await Promise.resolve();
    });
    const attach = container.querySelector<HTMLButtonElement>('button[aria-label="Attach files"]')!;
    expect(attach.closest(".relative")).not.toBeNull();
    await act(async () => {
      container.querySelector<HTMLButtonElement>('button[aria-label="Send message"]')!.click();
      await Promise.resolve();
      await Promise.resolve();
    });
    expect(onSend).toHaveBeenCalledWith(asset.ref, 0);
  });
});
