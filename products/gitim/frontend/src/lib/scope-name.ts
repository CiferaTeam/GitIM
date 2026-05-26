/**
 * Shared helpers for translating between the user-facing display form of
 * channels/DMs and the daemon's wire form, plus a couple of route shape
 * predicates that travel together.
 *
 * Display form is what the sidebar / message header renders; wire form is
 * what the daemon API expects. The translation is asymmetric only for DMs:
 *
 *   `"general"`         ↔ `"general"`         (plain channel: passthrough)
 *   `"alice--lewis"`    ↔ `"dm:alice,lewis"`  (DM stem ↔ daemon DM key)
 *
 * The conventions live in one module so chat-layout, poll-loop, and future
 * call sites can't drift on the rules independently.
 */

/** "dm:alice,lewis" → "alice--lewis"; passthrough for channels. */
export function apiToDisplay(channel: string): string {
  if (channel.startsWith("dm:")) {
    return channel.slice(3).replace(",", "--");
  }
  return channel;
}

/** "alice--lewis" → "dm:alice,lewis"; passthrough for channels. */
export function toApiChannel(displayName: string): string {
  if (displayName.includes("--")) {
    return `dm:${displayName.split("--").join(",")}`;
  }
  return displayName;
}

/** Tolerant percent-decode — invalid escapes fall back to the raw segment
 *  rather than throwing, so a malformed URL never crashes the route parser. */
export function decodePathSegment(segment: string): string {
  try {
    return decodeURIComponent(segment);
  } catch {
    return segment;
  }
}

export function parseCardRoute(
  pathname: string,
): { channel: string; cardId: string } | null {
  const match = /^\/cards\/([^/]+)\/([^/]+)\/?$/.exec(pathname);
  if (!match) return null;
  return {
    channel: decodePathSegment(match[1]),
    cardId: decodePathSegment(match[2]),
  };
}

/** Daemon contract: an unknown / removed workspace responds with `ok=false`
 *  and `error === "unknown workspace"`. Callers use this to trigger a
 *  workspace refresh instead of treating it as a transport failure. */
export function isUnknownWorkspaceResponse(res: {
  ok: boolean;
  error?: string | null;
}): boolean {
  return !res.ok && res.error === "unknown workspace";
}

export function isChatRoute(pathname: string): boolean {
  return pathname === "/chat" || pathname.startsWith("/chat/");
}
