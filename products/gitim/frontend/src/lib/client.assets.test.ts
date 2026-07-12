// @vitest-environment jsdom

import { beforeEach, describe, expect, it, vi } from "vitest";
import { parseAssetRef, type AssetRef } from "./asset-ref";

function createMemoryStorage(): Storage {
  const values = new Map<string, string>();
  return {
    get length() { return values.size; },
    clear() { values.clear(); },
    getItem(key: string) { return values.get(key) ?? null; },
    key(index: number) { return Array.from(values.keys())[index] ?? null; },
    removeItem(key: string) { values.delete(key); },
    setItem(key: string, value: string) { values.set(key, value); },
  };
}

Object.defineProperty(globalThis, "localStorage", {
  configurable: true,
  value: createMemoryStorage(),
});

vi.mock("./backend", () => ({
  HttpBackend: class {
    constructor(baseUrl: () => string) { void baseUrl; }
  },
  LocalBackend: class {
    constructor(config: unknown) { void config; }
  },
}));

vi.mock("@isomorphic-git/lightning-fs", () => ({ default: class {} }));

const RUNTIME_BASE = "http://127.0.0.1:9999";
const ORIGIN = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const HASH = "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88";
const VALID_REF = `<^v1/${ORIGIN}/sha256:${HASH}?name=fleet-assets.png&type=image%2Fpng&size=184203&width=1600&height=900>`;

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "Content-Type": "application/json" },
  });
}

async function setup(mode: "remote" | "local" = "remote"): Promise<typeof import("./client")> {
  vi.resetModules();
  const { useConnectionStore } = await import("@/hooks/use-connection-store");
  useConnectionStore.setState({ mode, port: mode === "remote" ? 9999 : null, status: "ready" });
  return import("./client");
}

function sizedFile(name: string, size: number): File {
  const file = new File(["x"], name, { type: "application/octet-stream" });
  Object.defineProperty(file, "size", { configurable: true, value: size });
  return file;
}

function successBody() {
  return {
    ok: true,
    assets: [{
      ref: VALID_REF,
      sha256: HASH,
      name: "fleet-assets.png",
      media_type: "image/png",
      size: 184203,
      width: 1600,
      height: 900,
    }],
  };
}

describe("uploadAssets", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    localStorage.clear();
  });

  it("uploads repeated file fields in caller order to the encoded workspace URL", async () => {
    const client = await setup();
    const fileA = new File(["a"], "a.txt");
    const fileB = new File(["b"], "b.txt");
    let capturedUrl = "";
    let capturedInit: RequestInit | undefined;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (input, init) => {
      capturedUrl = String(input);
      capturedInit = init;
      const body = successBody();
      return jsonResponse(200, { ...body, assets: [body.assets[0], body.assets[0]] });
    });

    const result = await client.uploadAssets("room/一", [fileA, fileB]);

    expect(result.ok).toBe(true);
    expect(capturedUrl).toBe(`${RUNTIME_BASE}/workspaces/room%2F%E4%B8%80/assets`);
    expect(capturedInit?.method).toBe("POST");
    expect((capturedInit?.body as FormData).getAll("file")).toEqual([fileA, fileB]);
  });

  it("propagates the optional AbortSignal", async () => {
    const client = await setup();
    const controller = new AbortController();
    let capturedInit: RequestInit | undefined;
    vi.spyOn(globalThis, "fetch").mockImplementation(async (_input, init) => {
      capturedInit = init;
      return jsonResponse(200, successBody());
    });
    await client.uploadAssets("room", [new File(["a"], "a.txt")], controller.signal);
    expect(capturedInit?.signal).toBe(controller.signal);
  });

  it("normalizes a complete Runtime success body", async () => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse(200, successBody()));
    const result = await client.uploadAssets("room", [new File(["a"], "a")]);
    expect(result).toEqual({
      ok: true,
      data: {
        assets: [{
          version: 1,
          originRuntimeId: ORIGIN,
          sha256: HASH,
          name: "fleet-assets.png",
          mediaType: "image/png",
          size: 184203,
          width: 1600,
          height: 900,
          raw: VALID_REF,
          ref: VALID_REF,
        }],
      },
    });
  });

  it("preserves Runtime errors and error codes", async () => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse(413, {
      ok: false,
      error: "asset request exceeds the 200 MiB limit",
      error_code: "request_too_large",
    }));
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).resolves.toEqual({
      ok: false,
      error: "asset request exceeds the 200 MiB limit",
      error_code: "request_too_large",
    });
  });

  it("normalizes network failures", async () => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockRejectedValue(new TypeError("Failed to fetch"));
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).resolves.toEqual({
      ok: false,
      error: "Failed to fetch",
    });
  });

  it("preserves AbortError semantics", async () => {
    const client = await setup();
    const error = new DOMException("aborted", "AbortError");
    vi.spyOn(globalThis, "fetch").mockRejectedValue(error);
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).rejects.toBe(error);
  });

  it("preserves AbortError while reading the response body", async () => {
    const client = await setup();
    const error = new DOMException("aborted", "AbortError");
    vi.spyOn(globalThis, "fetch").mockResolvedValue({
      ok: true,
      status: 200,
      json: vi.fn().mockRejectedValue(error),
    } as unknown as Response);
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).rejects.toBe(error);
  });

  it("normalizes ordinary response body parse failures", async () => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockResolvedValue({
      ok: true,
      status: 200,
      json: vi.fn().mockRejectedValue(new SyntaxError("invalid JSON")),
    } as unknown as Response);
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).resolves.toMatchObject({
      ok: false,
      error_code: "invalid_response",
    });
  });

  it.each([
    ["missing assets", { ok: true }],
    ["noncanonical ref", { ok: true, assets: [{ ...successBody().assets[0], ref: "invalid" }] }],
    ["inconsistent hash", { ok: true, assets: [{ ...successBody().assets[0], sha256: "0".repeat(64) }] }],
    ["inconsistent dimensions", { ok: true, assets: [{ ...successBody().assets[0], width: 10 }] }],
  ])("rejects malformed 2xx responses: %s", async (_label, body) => {
    const client = await setup();
    vi.spyOn(globalThis, "fetch").mockResolvedValue(jsonResponse(200, body));
    const result = await client.uploadAssets("room", [new File(["a"], "a.txt")]);
    expect(result.ok).toBe(false);
    expect(result.error_code).toBe("invalid_response");
  });

  it.each([
    ["an empty filename", ""],
    ["a control-only filename", "\u0000\n"],
    ["a long raw path with a short basename", `${"x".repeat(300)}/a`],
    ["a one-byte filename", "a"],
    ["a normal filename", "report.txt"],
  ])("allows %s to reach Runtime normalization", async (_label, name) => {
    const client = await setup();
    const fetchSpy = vi.spyOn(globalThis, "fetch").mockResolvedValue(
      jsonResponse(200, successBody()),
    );
    const result = await client.uploadAssets("room", [new File(["x"], name)]);
    expect(result.ok).toBe(true);
    expect(fetchSpy).toHaveBeenCalledOnce();
  });

  it.each([
    ["zero files", []],
    ["more than ten files", Array.from({ length: 11 }, (_, i) => sizedFile(`${i}.txt`, 1))],
    ["a file over 50 MiB", [sizedFile("large.bin", 50 * 1024 * 1024 + 1)]],
    ["an aggregate over 200 MiB", Array.from({ length: 5 }, (_, i) => sizedFile(`${i}.bin`, 50 * 1024 * 1024))],
    ["a filename over 255 UTF-8 bytes", [sizedFile(`${"é".repeat(128)}.txt`, 1)]],
  ])("rejects %s before fetch", async (_label, files) => {
    const client = await setup();
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    const result = await client.uploadAssets("room", files);
    expect(result.ok).toBe(false);
    expect(result.error_code).toBe("invalid_upload");
    expect(fetchSpy).not.toHaveBeenCalled();
  });

  it("returns runtime_required in Browser/WASM mode without fetch", async () => {
    const client = await setup("local");
    const fetchSpy = vi.spyOn(globalThis, "fetch");
    await expect(client.uploadAssets("room", [new File(["a"], "a.txt")])).resolves.toMatchObject({
      ok: false,
      error_code: "runtime_required",
    });
    expect(fetchSpy).not.toHaveBeenCalled();
  });
});

describe("assetResolveUrl", () => {
  it("uses the current local Runtime base URL and canonical asset fields", async () => {
    const client = await setup();
    const parsed = parseAssetRef(VALID_REF)!;
    expect(client.assetResolveUrl("room", parsed)).toBe(
      `${RUNTIME_BASE}/workspaces/room/assets/resolve/${ORIGIN}/${HASH}?name=fleet-assets.png`,
    );
    expect(client.assetResolveUrl("room", parsed, { download: true })).toBe(
      `${RUNTIME_BASE}/workspaces/room/assets/resolve/${ORIGIN}/${HASH}?name=fleet-assets.png&download=1`,
    );
  });

  it("encodes every dynamic path segment and a sanitized filename", async () => {
    const client = await setup();
    const parsed = parseAssetRef(VALID_REF)!;
    const unsafe = {
      ...parsed,
      originRuntimeId: "peer/origin",
      sha256: "hash?value",
      name: "folder\\line\u0000报告 #1+%.txt",
    } as AssetRef;
    expect(client.assetResolveUrl("room/一", unsafe)).toBe(
      `${RUNTIME_BASE}/workspaces/room%2F%E4%B8%80/assets/resolve/peer%2Forigin/hash%3Fvalue?name=line%E6%8A%A5%E5%91%8A%20%231%2B%25.txt`,
    );
  });
});
