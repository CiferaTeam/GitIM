/**
 * Normalizes a browser-supplied filename for asset references, upload
 * validation, and display: strips any path prefix, removes control
 * characters, and falls back to a placeholder when nothing usable remains.
 * Single source of truth shared by the attachment draft store, the upload
 * client, and the draft strip.
 */
export function normalizedFilename(name: string): string {
  const basename = name.split(/[\\/]/).at(-1) ?? "";
  const cleaned = Array.from(basename)
    .filter((character) => !/\p{Cc}/u.test(character))
    .join("");
  return cleaned || "attachment";
}

/**
 * Formats a byte count in binary (IEC) units, matching the asset size labels
 * shown across chat surfaces. Single source of truth shared by the asset
 * fragment and the attachment draft strip.
 */
export function formatBinarySize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(bytes < 10 * 1024 ? 1 : 0)} KiB`;
  }
  return `${(bytes / (1024 * 1024)).toFixed(bytes < 10 * 1024 * 1024 ? 1 : 0)} MiB`;
}
