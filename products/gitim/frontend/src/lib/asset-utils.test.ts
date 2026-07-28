import { describe, expect, it } from "vitest";

import { formatBinarySize, normalizedFilename } from "./asset-utils";

describe("normalizedFilename", () => {
  it.each([
    ["a plain name", "report.txt", "report.txt"],
    ["a POSIX path", "/tmp/dir/report.txt", "report.txt"],
    ["a Windows path", "C:\\dir\\report.txt", "report.txt"],
    ["a trailing separator", "dir/", "attachment"],
    ["control characters", "re\u0000port\n.txt", "report.txt"],
    ["unicode and punctuation", "报告 #1+%.txt", "报告 #1+%.txt"],
    ["an empty name", "", "attachment"],
    ["a control-only name", "\u0000\n", "attachment"],
    ["only separators", "\\/", "attachment"],
  ])("normalizes %s", (_label, input, expected) => {
    expect(normalizedFilename(input)).toBe(expected);
  });
});

describe("formatBinarySize", () => {
  it.each([
    [0, "0 B"],
    [512, "512 B"],
    [1023, "1023 B"],
    [1024, "1.0 KiB"],
    [1536, "1.5 KiB"],
    [10 * 1024, "10 KiB"],
    [184203, "180 KiB"],
    [1024 * 1024, "1.0 MiB"],
    [2 * 1024 * 1024, "2.0 MiB"],
    [10 * 1024 * 1024, "10 MiB"],
    [200 * 1024 * 1024, "200 MiB"],
  ])("formats %i bytes as %s", (bytes, expected) => {
    expect(formatBinarySize(bytes)).toBe(expected);
  });
});
