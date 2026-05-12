/** Page size for both initial channel load and history paging. Single
 *  source of truth: changing this here changes every read call. */
export const MESSAGES_PAGE_SIZE = 50;

export type LoadOlderDecision =
  | { kind: "fetch"; since: number }
  | { kind: "skip"; reason: "no_messages" | "at_top" };

/** Decide whether to fetch older messages, and with what `since`.
 *
 *  Pure function, isolated for unit testing. The caller (chat-layout's
 *  handleLoadOlder) wires this to the client.read call, the store update,
 *  and the in-flight guard.
 *
 *  Sentinel:
 *    `oldestLine === undefined` → no messages yet on screen, nothing to page
 *    `oldestLine <= 1`          → already at the top of the channel
 *
 *  Otherwise: `since = max(0, oldestLine - pageSize - 1)`. This is the
 *  smallest cursor that, when daemon retains `line > since` and head-cuts
 *  to `pageSize`, returns exactly `[oldestLine - pageSize .. oldestLine - 1]`.
 *  When `oldestLine - pageSize - 1 < 0` we clamp to 0 so the daemon returns
 *  whatever is left (the caller then sets `hasMoreHistory = false` because
 *  fewer than pageSize entries come back).
 */
export function computeLoadOlderSince(
  oldestLine: number | undefined,
  pageSize: number,
): LoadOlderDecision {
  if (oldestLine === undefined) return { kind: "skip", reason: "no_messages" };
  if (oldestLine <= 1) return { kind: "skip", reason: "at_top" };
  const since = Math.max(0, oldestLine - pageSize - 1);
  return { kind: "fetch", since };
}
