import type { SessionRefDragPayload } from "./types";

export const SESSION_REF_MIME = "application/x-gitim-session-ref";

export { type SessionRefDragPayload };

export function dataTransferHasSessionRef(
  types: DOMStringList | readonly string[],
): boolean {
  if ("contains" in types) {
    return types.contains(SESSION_REF_MIME);
  }
  return Array.from(types).includes(SESSION_REF_MIME);
}

export function readSessionRefDragPayload(
  dataTransfer: DataTransfer,
): SessionRefDragPayload | null {
  const raw = dataTransfer.getData(SESSION_REF_MIME);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Partial<SessionRefDragPayload>;
    if (
      typeof parsed.id !== "string" ||
      typeof parsed.title !== "string" ||
      typeof parsed.agent !== "string" ||
      typeof parsed.ref !== "string"
    ) {
      return null;
    }
    return {
      id: parsed.id,
      title: parsed.title,
      agent: parsed.agent,
      ref: parsed.ref,
    };
  } catch {
    return null;
  }
}

/** Parse a `session:qs-<ulid>(:L<line>)?` reference from text. */
export function parseSessionRef(
  text: string,
): { sessionId: string; lineNumber?: number } | null {
  const match = text.match(
    /\bsession:(qs-[0-9A-HJKMNP-TV-Z]{26})(:L(\d{6,}))?\b/,
  );
  if (!match) return null;
  const sessionId = match[1];
  const lineNumber = match[3] ? parseInt(match[3], 10) : undefined;
  return { sessionId, lineNumber };
}
