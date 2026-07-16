// @vitest-environment jsdom

import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { formatAssetRef } from "@/lib/asset-ref";
import type { UploadedAsset } from "@/lib/client";
import {
  attachmentDraftKey,
  type AttachmentDraft,
  type PendingAttachment,
  useAttachmentDraftStore,
} from "./use-attachment-draft-store";

const ORIGIN = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const HASH = "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88";
const MIB = 1024 * 1024;
const ORIGINAL_CREATE_OBJECT_URL = Object.getOwnPropertyDescriptor(URL, "createObjectURL");
const ORIGINAL_REVOKE_OBJECT_URL = Object.getOwnPropertyDescriptor(URL, "revokeObjectURL");

function file(
  name: string,
  {
    size = 1,
    type = "application/octet-stream",
    lastModified = 1,
  }: { size?: number; type?: string; lastModified?: number } = {},
): File {
  const value = new File(["x"], name, { type, lastModified });
  Object.defineProperty(value, "size", { configurable: true, value: size });
  return value;
}

function png(name: string, options: { size?: number; lastModified?: number } = {}): File {
  return file(name, { ...options, type: "image/png" });
}

function uploaded(name: string, size = 1): UploadedAsset {
  const fields = {
    version: 1 as const,
    originRuntimeId: ORIGIN,
    sha256: HASH,
    name,
    mediaType: "application/octet-stream",
    size,
  };
  const ref = formatAssetRef(fields);
  return { ...fields, raw: ref, ref };
}

function store() {
  return useAttachmentDraftStore.getState();
}

describe("attachmentDraftKey", () => {
  it("isolates workspaces and scopes with an injective tuple encoding", () => {
    const keys = [
      attachmentDraftKey("a:b", "c"),
      attachmentDraftKey("a", "b:c"),
      attachmentDraftKey("github.com/一/room", "channel/general"),
      attachmentDraftKey("github.com/一", "room/channel/general"),
    ];
    expect(new Set(keys)).toHaveProperty("size", keys.length);

    const channel = attachmentDraftKey("github.com/a/room", "general");
    const dm = attachmentDraftKey("github.com/a/room", "dm:alice");
    store().addFiles(channel, [png("channel.png")]);
    store().addFiles(dm, [png("dm.png")]);

    expect(store().drafts[channel].items[0].file.name).toBe("channel.png");
    expect(store().drafts[dm].items[0].file.name).toBe("dm.png");
  });
});

describe("attachment draft selection", () => {
  it("accepts in caller order, rejects existing and in-batch duplicates, and keeps valid files", () => {
    const key = attachmentDraftKey("workspace", "channel:general");
    const first = file("first.txt", { lastModified: 10 });
    const duplicateFirst = file("first.txt", { lastModified: 10 });
    const tooLarge = file("large.bin", { size: 50 * MIB + 1 });
    const second = file("second.txt", { lastModified: 20 });

    const initial = store().addFiles(key, [first]);
    const result = store().addFiles(key, [duplicateFirst, tooLarge, second, second]);

    expect(result.accepted.map((item) => item.file.name)).toEqual(["second.txt"]);
    expect(result.rejected.map((rejection) => rejection.code)).toEqual([
      "duplicate",
      "file_too_large",
      "duplicate",
    ]);
    expect(result.rejected.map((rejection) => rejection.file)).toEqual([
      duplicateFirst,
      tooLarge,
      second,
    ]);
    expect(store().drafts[key].items.map((item) => item.file.name)).toEqual([
      "first.txt",
      "second.txt",
    ]);
    expect(initial.accepted[0].id).toBe(store().drafts[key].items[0].id);
    expect(store().drafts[key]).toMatchObject({ status: "error" });
    expect(store().drafts[key].error).toBe(result.rejected[0].message);
  });

  it("applies count limits incrementally at exactly ten items", () => {
    const key = attachmentDraftKey("workspace", "count");
    const files = Array.from({ length: 11 }, (_, index) =>
      file(`${index}.txt`, { lastModified: index }),
    );

    const result = store().addFiles(key, files);

    expect(result.accepted).toHaveLength(10);
    expect(result.accepted.map((item) => item.file.name)).toEqual(
      files.slice(0, 10).map((item) => item.name),
    );
    expect(result.rejected).toMatchObject([{ code: "too_many_items", file: files[10] }]);
  });

  it("accepts the per-file and aggregate byte boundaries, including zero bytes", () => {
    const perFileKey = attachmentDraftKey("workspace", "per-file");
    const perFile = store().addFiles(perFileKey, [
      file("zero.bin", { size: 0 }),
      file("fifty.bin", { size: 50 * MIB }),
      file("over.bin", { size: 50 * MIB + 1 }),
    ]);
    expect(perFile.accepted.map((item) => item.file.name)).toEqual(["zero.bin", "fifty.bin"]);
    expect(perFile.rejected).toMatchObject([{ code: "file_too_large" }]);

    const aggregateKey = attachmentDraftKey("workspace", "aggregate");
    const aggregate = store().addFiles(aggregateKey, [
      file("a.bin", { size: 50 * MIB }),
      file("b.bin", { size: 50 * MIB }),
      file("c.bin", { size: 50 * MIB }),
      file("d.bin", { size: 50 * MIB }),
      file("e.bin", { size: 1 }),
    ]);
    expect(aggregate.accepted).toHaveLength(4);
    expect(aggregate.rejected).toMatchObject([{ code: "aggregate_too_large" }]);
    expect(store().drafts[aggregateKey].items.reduce((sum, item) => sum + item.file.size, 0)).toBe(
      200 * MIB,
    );
  });

  it("validates the Runtime-normalized basename and the 255-byte boundary", () => {
    const key = attachmentDraftKey("workspace", "names");
    const atBoundary = file(`folder\\linea\u0000${"é".repeat(123)}.txt`, { lastModified: 1 });
    const overBoundary = file(`${"é".repeat(126)}.txt`, { lastModified: 2 });
    const fallback = file("folder/\u0000\n", { lastModified: 3 });

    const result = store().addFiles(key, [atBoundary, overBoundary, fallback]);

    expect(new TextEncoder().encode(`linea${"é".repeat(123)}.txt`)).toHaveLength(255);
    expect(result.accepted.map((item) => item.file)).toEqual([atBoundary, fallback]);
    expect(result.rejected).toMatchObject([{ code: "filename_too_long", file: overBoundary }]);
  });

  it("preflights the longest canonical Runtime reference variant", () => {
    const key = attachmentDraftKey("workspace", "ref-boundary");
    const maximumExpansionName = "!".repeat(255);
    const result = store().addFiles(key, [file(maximumExpansionName, { size: 50 * MIB })]);
    const worstCase = formatAssetRef({
      version: 1,
      originRuntimeId: ORIGIN,
      sha256: HASH,
      name: maximumExpansionName,
      mediaType: "image/jpeg",
      size: 50 * MIB,
      width: 0xffff_ffff,
      height: 0xffff_ffff,
    });

    expect(new TextEncoder().encode(worstCase).length).toBeLessThanOrEqual(1024);
    expect(result.accepted).toHaveLength(1);
    expect(result.rejected).toEqual([]);
  });

  it("creates previews only for approved raster MIME values", () => {
    const key = attachmentDraftKey("workspace", "previews");
    const rasterTypes = ["image/png", "image/jpeg", "image/gif", "image/webp", "image/avif"];
    const uppercaseRaster = file("upper.png", { type: "image/png", lastModified: 11 });
    Object.defineProperty(uppercaseRaster, "type", { configurable: true, value: "IMAGE/PNG" });
    const files = [
      ...rasterTypes.map((type, index) => file(`${index}.img`, { type, lastModified: index })),
      file("svg.svg", { type: "image/svg+xml", lastModified: 10 }),
      uppercaseRaster,
      file("rejected.png", { type: "image/png", size: 50 * MIB + 1, lastModified: 12 }),
    ];

    const result = store().addFiles(key, files);

    expect(result.accepted.map((item) => item.previewUrl)).toEqual([
      "blob:0.img",
      "blob:1.img",
      "blob:2.img",
      "blob:3.img",
      "blob:4.img",
      undefined,
      undefined,
    ]);
    expect(URL.createObjectURL).toHaveBeenCalledTimes(5);
  });

  it("accepts raster files when object URL creation is unavailable or throws", () => {
    const key = attachmentDraftKey("workspace", "preview-failures");
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: undefined,
    });
    const unavailable = store().addFiles(key, [png("unavailable.png")]);
    Object.defineProperty(URL, "createObjectURL", {
      configurable: true,
      value: vi.fn(() => {
        throw new Error("object URL unavailable");
      }),
    });
    const throwing = store().addFiles(key, [png("throwing.png", { lastModified: 2 })]);

    expect(unavailable.accepted[0].previewUrl).toBeUndefined();
    expect(throwing.accepted[0].previewUrl).toBeUndefined();
    expect(store().drafts[key].items).toHaveLength(2);
  });

  it("publishes frozen readonly draft state and accepted selections", () => {
    const key = attachmentDraftKey("workspace", "readonly");
    const result = store().addFiles(key, [png("readonly.png")]);
    const drafts = store().drafts;
    const draft = drafts[key];
    const originalId = draft.items[0].id;

    expect(Object.isFrozen(drafts)).toBe(true);
    expect(Object.isFrozen(draft)).toBe(true);
    expect(Object.isFrozen(draft.items)).toBe(true);
    expect(Object.isFrozen(draft.items[0])).toBe(true);
    expect(Object.isFrozen(result.accepted)).toBe(true);
    expect(result.accepted[0]).toBe(draft.items[0]);

    expect(() => {
      (drafts as Record<string, AttachmentDraft>).other = draft;
    }).toThrow(TypeError);
    expect(() => {
      (draft as { status: string }).status = "error";
    }).toThrow(TypeError);
    expect(() => {
      (draft.items as PendingAttachment[]).push(result.accepted[0]);
    }).toThrow(TypeError);
    expect(() => {
      (draft.items[0] as { id: string }).id = "mutated";
    }).toThrow(TypeError);
    expect(() => {
      (result.accepted as PendingAttachment[]).pop();
    }).toThrow(TypeError);

    expect(store().drafts[key]).toBe(draft);
    expect(store().drafts[key].items).toHaveLength(1);
    expect(store().drafts[key].items[0].id).toBe(originalId);
  });
});

describe("attachment draft operations", () => {
  it("rejects selection and removal while uploading or sending", () => {
    const key = attachmentDraftKey("workspace", "busy");
    const added = store().addFiles(key, [file("first.txt")]);
    const operation = store().beginOperation(key)!;

    expect(store().addFiles(key, [file("second.txt")]).rejected).toMatchObject([
      { code: "busy" },
    ]);
    expect(store().removeItem(key, added.accepted[0].id)).toBe(false);

    expect(
      store().markUploaded(key, operation.generation, [
        { id: operation.items[0].id, asset: uploaded("first.txt") },
      ]),
    ).toBe(true);
    expect(store().markSending(key, operation.generation)).toBe(true);
    expect(store().addFiles(key, [file("third.txt")]).rejected).toMatchObject([
      { code: "busy" },
    ]);
    expect(store().removeItem(key, added.accepted[0].id)).toBe(false);
  });

  it("captures an immutable operation snapshot and never reuses generations", () => {
    const key = attachmentDraftKey("workspace", "generation");
    store().addFiles(key, [file("old.txt")]);
    const first = store().beginOperation(key)!;

    expect(Object.isFrozen(first)).toBe(true);
    expect(Object.isFrozen(first.items)).toBe(true);
    expect(Object.isFrozen(first.items[0])).toBe(true);
    expect(first.items[0]).not.toBe(store().drafts[key].items[0]);
    store().disposeWorkspace("workspace");

    store().addFiles(key, [file("new.txt", { lastModified: 2 })]);
    const second = store().beginOperation(key)!;
    expect(second.generation).toBeGreaterThan(first.generation);
    expect(first.items[0].file.name).toBe("old.txt");
  });

  it("retains uploaded assets after send failure and while adding a new file", () => {
    const key = attachmentDraftKey("workspace", "retry");
    store().addFiles(key, [file("first.txt")]);
    const first = store().beginOperation(key)!;
    const firstAsset = uploaded("first.txt");
    expect(
      store().markUploaded(key, first.generation, [{ id: first.items[0].id, asset: firstAsset }]),
    ).toBe(true);
    expect(store().markSending(key, first.generation)).toBe(true);
    expect(store().failOperation(key, first.generation, "send failed")).toBe(true);

    const added = store().addFiles(key, [file("second.txt", { lastModified: 2 })]);
    expect(store().drafts[key].status).toBe("idle");
    expect(store().drafts[key].error).toBeUndefined();
    expect(store().drafts[key].items[0].uploaded).toEqual(firstAsset);

    const retry = store().beginOperation(key)!;
    expect(
      store().markUploaded(key, retry.generation, [
        { id: added.accepted[0].id, asset: uploaded("second.txt") },
      ]),
    ).toBe(true);
    expect(store().markSending(key, retry.generation)).toBe(true);
  });

  it("applies upload mappings atomically in selection order", () => {
    const key = attachmentDraftKey("workspace", "mapping");
    store().addFiles(key, [
      file("a.txt", { lastModified: 1 }),
      file("b.txt", { lastModified: 2 }),
      file("c.txt", { lastModified: 3 }),
    ]);
    const operation = store().beginOperation(key)!;
    const [a, b, c] = operation.items;
    const aAsset = uploaded("a.txt");
    const bAsset = uploaded("b.txt");
    const cAsset = uploaded("c.txt");

    expect(store().markUploaded(key, operation.generation + 1, [
      { id: a.id, asset: aAsset },
      { id: b.id, asset: bAsset },
      { id: c.id, asset: cAsset },
    ])).toBe(false);
    expect(store().markUploaded(key, operation.generation, [
      { id: a.id, asset: aAsset },
      { id: "unknown", asset: bAsset },
      { id: c.id, asset: cAsset },
    ])).toBe(false);
    expect(store().markUploaded(key, operation.generation, [
      { id: a.id, asset: aAsset },
      { id: a.id, asset: bAsset },
      { id: c.id, asset: cAsset },
    ])).toBe(false);
    expect(store().markUploaded(key, operation.generation, [
      { id: a.id, asset: aAsset },
      { id: b.id, asset: bAsset },
    ])).toBe(false);
    expect(store().drafts[key].items.every((item) => item.uploaded === undefined)).toBe(true);

    expect(store().markUploaded(key, operation.generation, [
      { id: c.id, asset: cAsset },
      { id: a.id, asset: aAsset },
      { id: b.id, asset: bAsset },
    ])).toBe(true);
    expect(store().drafts[key].items.map((item) => item.file.name)).toEqual([
      "a.txt",
      "b.txt",
      "c.txt",
    ]);
    expect(store().drafts[key].items.map((item) => item.uploaded)).toEqual([
      aAsset,
      bAsset,
      cAsset,
    ]);
  });

  it("rejects swapped upload mappings by normalized filename and size", () => {
    const key = attachmentDraftKey("workspace", "swapped-mapping");
    store().addFiles(key, [
      file("folder/a.txt", { size: 3, lastModified: 1 }),
      file("folder\\b.txt", { size: 7, lastModified: 2 }),
    ]);
    const operation = store().beginOperation(key)!;
    const [a, b] = operation.items;

    expect(store().markUploaded(key, operation.generation, [
      { id: a.id, asset: uploaded("b.txt", 7) },
      { id: b.id, asset: uploaded("a.txt", 3) },
    ])).toBe(false);
    expect(store().drafts[key].items.every((item) => item.uploaded === undefined)).toBe(true);
  });

  it("rejects a stale mismatched asset without applying valid sibling mappings", () => {
    const key = attachmentDraftKey("workspace", "stale-mapping");
    store().addFiles(key, [
      file("current.txt", { size: 4, lastModified: 1 }),
      file("sibling.txt", { size: 8, lastModified: 2 }),
    ]);
    const operation = store().beginOperation(key)!;
    const [current, sibling] = operation.items;

    expect(store().markUploaded(key, operation.generation, [
      { id: current.id, asset: uploaded("old.txt", 3) },
      { id: sibling.id, asset: uploaded("sibling.txt", 8) },
    ])).toBe(false);
    expect(store().drafts[key].items.every((item) => item.uploaded === undefined)).toBe(true);
  });

  it("marks sending only after every current item has an uploaded reference", () => {
    const key = attachmentDraftKey("workspace", "sending");
    store().addFiles(key, [file("a.txt"), file("b.txt", { lastModified: 2 })]);
    const operation = store().beginOperation(key)!;

    expect(store().markSending(key, operation.generation)).toBe(false);
    expect(store().markUploaded(key, operation.generation, operation.items.map((item) => ({
      id: item.id,
      asset: uploaded(item.file.name),
    })))).toBe(true);
    expect(store().markSending(key, operation.generation)).toBe(true);
    expect(store().drafts[key].status).toBe("sending");
  });

  it("updates only the captured scope key", () => {
    const channel = attachmentDraftKey("workspace", "channel");
    const dm = attachmentDraftKey("workspace", "dm:alice");
    store().addFiles(channel, [file("channel.txt")]);
    store().addFiles(dm, [file("dm.txt")]);
    const operation = store().beginOperation(channel)!;

    expect(store().markUploaded(channel, operation.generation, [
      { id: operation.items[0].id, asset: uploaded("channel.txt") },
    ])).toBe(true);
    expect(store().drafts[channel].items[0].uploaded).toBeDefined();
    expect(store().drafts[dm]).toMatchObject({ status: "idle" });
    expect(store().drafts[dm].items[0].uploaded).toBeUndefined();
  });
});

describe("attachment preview lifecycle", () => {
  it("disposes every scope for one workspace and invalidates failed-send operations", () => {
    const workspace = "runtime:room";
    const idleKey = attachmentDraftKey(workspace, "channel:general");
    const failedKey = attachmentDraftKey(workspace, "dm:alice");
    const otherKey = attachmentDraftKey("runtime:other", "channel:general");
    store().addFiles(idleKey, [png("idle.png")]);
    store().addFiles(failedKey, [png("uploaded.png")]);
    store().addFiles(otherKey, [png("other.png")]);
    const operation = store().beginOperation(failedKey)!;
    expect(store().markUploaded(failedKey, operation.generation, [{
      id: operation.items[0].id,
      asset: { ...uploaded("uploaded.png"), mediaType: "image/png" },
    }])).toBe(true);
    expect(store().markSending(failedKey, operation.generation)).toBe(true);
    expect(store().failOperation(failedKey, operation.generation, "send failed")).toBe(true);

    const disposeWorkspace = (
      store() as unknown as { disposeWorkspace: (workspaceKey: string) => void }
    ).disposeWorkspace;
    disposeWorkspace(workspace);

    expect(store().drafts[idleKey]).toBeUndefined();
    expect(store().drafts[failedKey]).toBeUndefined();
    expect(store().drafts[otherKey].items[0].file.name).toBe("other.png");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:idle.png");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith("blob:uploaded.png");
    expect(URL.revokeObjectURL).not.toHaveBeenCalledWith("blob:other.png");
    expect(store().failOperation(failedKey, operation.generation, "late")).toBe(false);

    store().addFiles(failedKey, [png("replacement.png", { lastModified: 2 })]);
    const replacement = store().beginOperation(failedKey)!;
    expect(replacement.generation).toBeGreaterThan(operation.generation);
  });

  it("revokes previews exactly once across removal, success, and disposal", () => {
    const removeKey = attachmentDraftKey("workspace", "remove");
    const successKey = attachmentDraftKey("workspace", "success");
    const disposeKey = attachmentDraftKey("workspace", "dispose");
    const removed = store().addFiles(removeKey, [png("removed.png")]).accepted[0];
    store().addFiles(successKey, [png("success.png")]);
    store().addFiles(disposeKey, [png("dispose.png")]);

    expect(store().removeItem(removeKey, removed.id)).toBe(true);
    expect(store().drafts[removeKey]).toBeUndefined();
    const operation = store().beginOperation(successKey)!;
    expect(store().markUploaded(successKey, operation.generation, [{
      id: operation.items[0].id,
      asset: uploaded("success.png"),
    }])).toBe(true);
    expect(store().markSending(successKey, operation.generation)).toBe(true);
    expect(store().completeSuccess(successKey, operation.generation)).toBe(true);
    store().disposeAll();
    store().disposeAll();

    expect(URL.revokeObjectURL).toHaveBeenCalledTimes(3);
    for (const name of [
      "removed.png",
      "success.png",
      "dispose.png",
    ]) {
      expect(URL.revokeObjectURL).toHaveBeenCalledWith(`blob:${name}`);
    }
    expect(store().drafts).toEqual({});
  });

  it("ignores stale success after disposal and recreation without touching the newer preview", () => {
    const key = attachmentDraftKey("workspace", "stale");
    const first = store().addFiles(key, [png("old.png")]);
    const operation = store().beginOperation(key)!;
    store().disposeWorkspace("workspace");
    store().addFiles(key, [png("new.png", { lastModified: 2 })]);

    expect(store().completeSuccess(key, operation.generation)).toBe(false);
    expect(store().failOperation(key, operation.generation, "late failure")).toBe(false);
    expect(store().drafts[key].items[0].file.name).toBe("new.png");
    expect(URL.revokeObjectURL).toHaveBeenCalledWith(first.accepted[0].previewUrl);
    expect(URL.revokeObjectURL).not.toHaveBeenCalledWith("blob:new.png");
  });

  it("finishes every cleanup transition when individual URL revocations throw", () => {
    const removeKey = attachmentDraftKey("workspace", "remove-throw");
    const successKey = attachmentDraftKey("workspace", "success-throw");
    const disposeKey = attachmentDraftKey("workspace", "dispose-throw");
    const removed = store().addFiles(removeKey, [png("remove-fail.png")]).accepted[0];
    store().addFiles(successKey, [
      png("success-fail.png"),
      png("success-ok.png", { lastModified: 2 }),
    ]);
    store().addFiles(disposeKey, [
      png("dispose-fail.png"),
      png("dispose-ok.png", { lastModified: 2 }),
    ]);
    const attempted: string[] = [];
    vi.mocked(URL.revokeObjectURL).mockImplementation((url) => {
      attempted.push(url);
      if (url.includes("fail")) throw new Error("revocation failed");
    });

    expect(store().removeItem(removeKey, removed.id)).toBe(true);
    expect(store().drafts[removeKey]).toBeUndefined();

    const operation = store().beginOperation(successKey)!;
    expect(store().markUploaded(successKey, operation.generation, operation.items.map((item) => ({
      id: item.id,
      asset: uploaded(item.file.name),
    })))).toBe(true);
    expect(store().markSending(successKey, operation.generation)).toBe(true);
    expect(store().completeSuccess(successKey, operation.generation)).toBe(true);
    expect(store().drafts[successKey]).toBeUndefined();

    store().disposeAll();
    store().disposeAll();
    expect(store().drafts).toEqual({});
    expect(attempted).toEqual([
      "blob:remove-fail.png",
      "blob:success-fail.png",
      "blob:success-ok.png",
      "blob:dispose-fail.png",
      "blob:dispose-ok.png",
    ]);
  });
});

beforeEach(() => {
  useAttachmentDraftStore.getState().disposeAll();
  Object.defineProperty(URL, "createObjectURL", {
    configurable: true,
    value: vi.fn((value: File) => `blob:${value.name}`),
  });
  Object.defineProperty(URL, "revokeObjectURL", {
    configurable: true,
    value: vi.fn(),
  });
});

afterEach(() => {
  useAttachmentDraftStore.getState().disposeAll();
  vi.restoreAllMocks();
});

afterAll(() => {
  for (const [name, descriptor] of [
    ["createObjectURL", ORIGINAL_CREATE_OBJECT_URL],
    ["revokeObjectURL", ORIGINAL_REVOKE_OBJECT_URL],
  ] as const) {
    if (descriptor === undefined) {
      delete (URL as unknown as Record<string, unknown>)[name];
    } else {
      Object.defineProperty(URL, name, descriptor);
    }
  }
});
