const CROCKFORD = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";
const QUICK_SESSION_ID_RE = /^qs-[0-9A-HJKMNP-TV-Z]{26}$/;
const QUICK_SESSION_REF_RE =
  /session:(qs-[0-9A-HJKMNP-TV-Z]{26})(?::L([0-9]{6,}))?/gu;

export interface QuickSessionRef {
  raw: string;
  sessionId: string;
  line?: number;
}

function isInvalidBoundary(character: string | undefined): boolean {
  return character !== undefined && /[\p{L}\p{N}_\-/\\]/u.test(character);
}

function encodeTime(timestamp: number): string {
  if (!Number.isSafeInteger(timestamp) || timestamp < 0 || timestamp > 0xffffffffffff) {
    throw new Error("Invalid ULID timestamp");
  }
  let remaining = timestamp;
  let encoded = "";
  for (let index = 0; index < 10; index += 1) {
    encoded = CROCKFORD[remaining % 32] + encoded;
    remaining = Math.floor(remaining / 32);
  }
  return encoded;
}

function encodeRandom(): string {
  if (typeof crypto === "undefined" || !crypto.getRandomValues) {
    throw new Error("Secure randomness is unavailable");
  }
  const bytes = crypto.getRandomValues(new Uint8Array(10));
  let bits = 0;
  let bitCount = 0;
  let encoded = "";
  for (const byte of bytes) {
    bits = (bits << 8) | byte;
    bitCount += 8;
    while (bitCount >= 5) {
      bitCount -= 5;
      encoded += CROCKFORD[(bits >>> bitCount) & 31];
      bits &= (1 << bitCount) - 1;
    }
  }
  return encoded;
}

function generateUlid(timestamp = Date.now()): string {
  return `${encodeTime(timestamp)}${encodeRandom()}`;
}

export function generateQuickSessionId(timestamp = Date.now()): string {
  return `qs-${generateUlid(timestamp)}`;
}

export function generateQuickSessionRequestId(timestamp = Date.now()): string {
  return generateUlid(timestamp);
}

export function formatQuickSessionRef(
  sessionId: string,
  line?: number,
): string {
  if (!QUICK_SESSION_ID_RE.test(sessionId)) {
    throw new Error("Invalid Quick Session id");
  }
  if (line === undefined) return `session:${sessionId}`;
  if (!Number.isSafeInteger(line) || line < 1) {
    throw new Error("Invalid Quick Session line");
  }
  return `session:${sessionId}:L${String(line).padStart(6, "0")}`;
}

export function extractQuickSessionRefs(text: string): QuickSessionRef[] {
  const refs: QuickSessionRef[] = [];
  for (const match of text.matchAll(QUICK_SESSION_REF_RE)) {
    const raw = match[0];
    const start = match.index;
    const end = start + raw.length;
    const before = Array.from(text.slice(0, start)).at(-1);
    const after = Array.from(text.slice(end))[0];
    if (
      isInvalidBoundary(before) ||
      isInvalidBoundary(after) ||
      text.slice(end).startsWith(":L")
    ) {
      continue;
    }
    const lineText = match[2];
    const line = lineText === undefined ? undefined : Number(lineText);
    if (line !== undefined && (!Number.isSafeInteger(line) || line < 1)) {
      continue;
    }
    refs.push({
      raw,
      sessionId: match[1],
      ...(line !== undefined ? { line } : {}),
    });
  }
  return refs;
}

export function parseQuickSessionRef(value: string): QuickSessionRef | null {
  const refs = extractQuickSessionRefs(value);
  return refs.length === 1 && refs[0].raw === value ? refs[0] : null;
}
