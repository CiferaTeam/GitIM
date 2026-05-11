import { describe, expect, it } from "vitest";
import { formatTokens } from "./format-tokens";

describe("formatTokens", () => {
  it("renders raw integers under 1000", () => {
    expect(formatTokens(0)).toBe("0");
    expect(formatTokens(1)).toBe("1");
    expect(formatTokens(999)).toBe("999");
  });

  it("renders thousands with K suffix and trims trailing zero", () => {
    expect(formatTokens(1_000)).toBe("1K");
    expect(formatTokens(1_500)).toBe("1.5K");
    expect(formatTokens(12_345)).toBe("12.3K");
    expect(formatTokens(999_999)).toBe("1000K");
  });

  it("renders millions with M suffix", () => {
    expect(formatTokens(1_000_000)).toBe("1M");
    expect(formatTokens(1_500_000)).toBe("1.5M");
    expect(formatTokens(12_300_000)).toBe("12.3M");
  });

  it("renders billions with B suffix", () => {
    expect(formatTokens(1_000_000_000)).toBe("1B");
    expect(formatTokens(1_500_000_000)).toBe("1.5B");
  });

  it("handles negatives with a leading minus", () => {
    expect(formatTokens(-1)).toBe("-1");
    expect(formatTokens(-12_345)).toBe("-12.3K");
  });

  it("renders non-finite as a placeholder rather than NaN/Infinity", () => {
    expect(formatTokens(Number.NaN)).toBe("—");
    expect(formatTokens(Number.POSITIVE_INFINITY)).toBe("—");
  });
});
