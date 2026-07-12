import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { formatAssetRef, parseAssetRef, type AssetRef } from "./asset-ref";

const fixtures = JSON.parse(
  readFileSync(
    new URL("../../../../../testdata/protocol/asset_refs_v1.json", import.meta.url),
    "utf8",
  ),
) as {
  valid: Array<{
    raw: string;
    name: string;
    media_type: string;
    size: number;
    width: number | null;
    height: number | null;
  }>;
  invalid: string[];
};

const ORIGIN = "3c6a295e-744a-41dc-ba60-5c21bb94e5a2";
const HASH = "8f2c4d7d7e931a62c18f6f24c8e388d72524d4c4cd6f88e9538f7d4a66c72a88";

function ref(
  query: string,
  originRuntimeId = ORIGIN,
  sha256 = HASH,
): string {
  return `<^v1/${originRuntimeId}/sha256:${sha256}?${query}>`;
}

function asset(overrides: Partial<Omit<AssetRef, "raw">> = {}): Omit<AssetRef, "raw"> {
  return {
    version: 1,
    originRuntimeId: ORIGIN,
    sha256: HASH,
    name: "report.txt",
    mediaType: "text/plain",
    size: 7,
    ...overrides,
  };
}

describe("asset reference protocol fixtures", () => {
  it.each(fixtures.valid)("parses and canonically formats $raw", (fixture) => {
    const parsed = parseAssetRef(fixture.raw);
    expect(parsed).not.toBeNull();
    expect(parsed).toMatchObject({
      version: 1,
      originRuntimeId: ORIGIN,
      sha256: fixture.raw.match(/\/sha256:([^?]+)/)?.[1],
      name: fixture.name,
      mediaType: fixture.media_type,
      size: fixture.size,
      raw: fixture.raw,
    });
    expect(parsed?.width).toBe(fixture.width ?? undefined);
    expect(parsed?.height).toBe(fixture.height ?? undefined);
    const { raw, ...withoutRaw } = parsed!;
    expect(raw).toBe(fixture.raw);
    expect(formatAssetRef(withoutRaw)).toBe(fixture.raw);
  });

  it.each(fixtures.invalid)("rejects invalid fixture %s", (raw) => {
    expect(parseAssetRef(raw)).toBeNull();
  });
});

describe("parseAssetRef", () => {
  it("accepts the UTF-8 byte boundaries for decoded fields", () => {
    const name = `${"é".repeat(127)}a`;
    const mediaType = `a/${"b".repeat(125)}`;
    expect(parseAssetRef(formatAssetRef(asset({ name })))).toMatchObject({ name });
    expect(parseAssetRef(formatAssetRef(asset({ mediaType })))).toMatchObject({ mediaType });
  });

  it("rejects decoded fields over their UTF-8 byte limits", () => {
    expect(parseAssetRef(ref(`name=${encodeURIComponent("é".repeat(128))}&type=text%2Fplain&size=1`))).toBeNull();
    expect(parseAssetRef(ref(`name=a&type=a%2F${"b".repeat(126)}&size=1`))).toBeNull();
  });

  it("rejects a canonical encoded reference over 1024 bytes", () => {
    const oversized = ref(
      `name=${"%21".repeat(255)}&type=a%2F${"%27".repeat(125)}&size=7`,
    );
    expect(new TextEncoder().encode(oversized).length).toBeGreaterThan(1024);
    expect(parseAssetRef(oversized)).toBeNull();
    expect(() =>
      formatAssetRef(asset({
        name: "!".repeat(255),
        mediaType: `a/${"'".repeat(125)}`,
      })),
    ).toThrow(/1024/);
  });

  it("accepts a canonical encoded reference at exactly 1024 bytes", () => {
    const raw = formatAssetRef(asset({
      name: `${"!".repeat(129)}${"a".repeat(125)}`,
      mediaType: `a/${"'".repeat(125)}`,
    }));
    expect(new TextEncoder().encode(raw).length).toBe(1024);
    expect(parseAssetRef(raw)?.raw).toBe(raw);
  });

  it("enforces canonical unsigned sizes and the 50 MiB limit", () => {
    expect(parseAssetRef(ref("name=a&type=text%2Fplain&size=52428800"))?.size).toBe(52_428_800);
    for (const size of ["52428801", "01", "+1", "-1", "1.0", "9007199254740992"]) {
      expect(parseAssetRef(ref(`name=a&type=text%2Fplain&size=${size}`))).toBeNull();
    }
  });

  it("enforces paired positive canonical u32 dimensions", () => {
    expect(
      parseAssetRef(ref("name=a&type=image%2Fpng&size=1&width=4294967295&height=1")),
    ).toMatchObject({ width: 4_294_967_295, height: 1 });
    for (const suffix of [
      "&width=1",
      "&height=1",
      "&width=0&height=1",
      "&width=1&height=4294967296",
      "&width=01&height=1",
    ]) {
      expect(parseAssetRef(ref(`name=a&type=image%2Fpng&size=1${suffix}`))).toBeNull();
    }
  });

  it("enforces exact query keys and canonical order", () => {
    for (const query of [
      "type=text%2Fplain&name=a&size=1",
      "name=a&size=1&type=text%2Fplain",
      "name=a&type=text%2Fplain&size=1&extra=x",
      "name=a&name=b&type=text%2Fplain&size=1",
      "name=a&type=text%2Fplain&type=text%2Fplain&size=1",
    ]) {
      expect(parseAssetRef(ref(query))).toBeNull();
    }
  });

  it("rejects unsafe decoded names", () => {
    for (const name of ["", "a%2Fb", "a%5Cb", "line%0Abreak", "tab%09name", "nul%00name"]) {
      expect(parseAssetRef(ref(`name=${name}&type=text%2Fplain&size=1`))).toBeNull();
    }
  });

  it("requires lowercase canonical UUID, hash, and media type", () => {
    expect(parseAssetRef(ref("name=a&type=Text%2FPlain&size=1"))).toBeNull();
    expect(parseAssetRef(ref("name=a&type=text%2Fplain&size=1", ORIGIN.toUpperCase()))).toBeNull();
    expect(parseAssetRef(ref("name=a&type=text%2Fplain&size=1", ORIGIN, HASH.toUpperCase()))).toBeNull();
  });

  it("requires the RFC token media-type grammar", () => {
    for (const type of ["text", "%2Fplain", "text%2F", "text%2Fplain%2Fextra", "text%2Fplain%3Bx", "text%2Fpl%40in"]) {
      expect(parseAssetRef(ref(`name=a&type=${type}&size=1`))).toBeNull();
    }
  });

  it("rejects malformed percent encoding and invalid UTF-8", () => {
    for (const name of ["%", "%2", "%GG", "%FF", "%C3%28"]) {
      expect(parseAssetRef(ref(`name=${name}&type=text%2Fplain&size=1`))).toBeNull();
    }
  });

  it("rejects noncanonical encoding variants", () => {
    for (const query of [
      "name=a&type=text%2fplain&size=1",
      "name=%61&type=text%2Fplain&size=1",
      "name=a!b&type=text%2Fplain&size=1",
      "name=a+b&type=text%2Fplain&size=1",
      "name=报告.txt&type=text%2Fplain&size=1",
    ]) {
      expect(parseAssetRef(ref(query))).toBeNull();
    }
  });

  it("rejects references with extra syntax or invalid version", () => {
    const canonical = formatAssetRef(asset());
    expect(parseAssetRef(`x${canonical}`)).toBeNull();
    expect(parseAssetRef(`${canonical}x`)).toBeNull();
    expect(parseAssetRef(canonical.replace("<^v1/", "<^v01/"))).toBeNull();
  });
});

describe("formatAssetRef", () => {
  it("uses uppercase RFC3986 percent escapes", () => {
    const raw = formatAssetRef(asset({ name: "résumé !'()*.txt", mediaType: "application/vnd.gitim+json" }));
    expect(raw).toContain("name=r%C3%A9sum%C3%A9%20%21%27%28%29%2A.txt");
    expect(raw).toContain("type=application%2Fvnd.gitim%2Bjson");
  });

  it("includes both dimensions in canonical order", () => {
    expect(formatAssetRef(asset({ width: 12, height: 34 }))).toContain(
      "&size=7&width=12&height=34>",
    );
  });

  it("throws instead of emitting invalid references", () => {
    const invalidAssets = [
      asset({ name: "../secret" }),
      asset({ mediaType: "Text/Plain" }),
      asset({ size: Number.MAX_SAFE_INTEGER }),
      asset({ width: 1 }),
      asset({ width: 0, height: 1 }),
      asset({ originRuntimeId: ORIGIN.toUpperCase() }),
    ];
    for (const invalid of invalidAssets) {
      expect(() => formatAssetRef(invalid)).toThrow(/invalid asset reference/i);
    }
  });
});
