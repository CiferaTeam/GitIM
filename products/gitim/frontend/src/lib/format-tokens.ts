/**
 * Format a token count into a human-friendly short string.
 *
 * Used by the token-usage cards/tags. The contract:
 *
 *   < 1_000           → bare integer ("0", "999")
 *   < 1_000_000       → "12.3K" with at most one decimal digit; trailing
 *                       ".0" is dropped so "1000" reads as "1K"
 *   < 1_000_000_000   → "1.2M" same convention
 *   ≥ 1_000_000_000   → "1.2B" same convention (futureproofing — totals
 *                       run for years, billions are reachable on long
 *                       deployments)
 *
 * Negative inputs are rendered with a leading minus; saturating arithmetic
 * elsewhere makes them unlikely, but the helper still handles them so a
 * stray sign doesn't blow up the UI.
 */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n)) return "—";
  const sign = n < 0 ? "-" : "";
  const abs = Math.abs(n);
  if (abs < 1_000) return `${sign}${abs}`;
  if (abs < 1_000_000) return `${sign}${trimTrailingZero(abs / 1_000)}K`;
  if (abs < 1_000_000_000) return `${sign}${trimTrailingZero(abs / 1_000_000)}M`;
  return `${sign}${trimTrailingZero(abs / 1_000_000_000)}B`;
}

/** "12.0" → "12", "12.3" → "12.3". */
function trimTrailingZero(n: number): string {
  const fixed = n.toFixed(1);
  return fixed.endsWith(".0") ? fixed.slice(0, -2) : fixed;
}
