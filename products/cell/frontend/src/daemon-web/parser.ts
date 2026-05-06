// TS port of gitim-core/src/parser.rs — parses .thread files into structured data.

export interface ParsedMessage {
  type: "message";
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;
  body: string;
}

export interface ParsedEvent {
  type: "event";
  line_number: number;
  point_to: number;
  author: string;
  timestamp: string;
  event_type: string;
  meta: Record<string, unknown>;
}

export type ThreadEntry = ParsedMessage | ParsedEvent;

export interface ThreadFile {
  entries: ThreadEntry[];
}

const MSG_RE =
  /^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9][a-z0-9-]*)\]\[(\d{8}T\d{6}Z)\](?:\[E:([^\]]+)\])?\s(.*)$/;

export function parseThread(text: string): ThreadFile {
  const input = text.replace(/\r\n/g, "\n");
  if (input.length === 0) {
    return { entries: [] };
  }

  const entries: ThreadEntry[] = [];
  let currentBody: string | null = null;

  // Rust's str::lines() doesn't yield a trailing empty string for a final '\n'.
  // JS split("\n") does, which would add a spurious continuation line. Match Rust behavior.
  const lines = input.endsWith("\n") ? input.slice(0, -1).split("\n") : input.split("\n");

  for (const line of lines) {
    const match = MSG_RE.exec(line);

    if (match) {
      // Finalize previous entry before starting a new one
      finalizeEntry(entries, currentBody);

      const lineNumber = parseInt(match[1], 10);
      const pointTo = parseInt(match[2], 10);
      const author = match[3];
      const timestamp = match[4];
      const eventType = match[5] ?? null;
      const bodyFirstLine = match[6];

      if (eventType !== null) {
        entries.push({
          type: "event",
          line_number: lineNumber,
          point_to: pointTo,
          author,
          timestamp,
          event_type: eventType,
          meta: {},
        });
      } else {
        entries.push({
          type: "message",
          line_number: lineNumber,
          point_to: pointTo,
          author,
          timestamp,
          body: "",
        });
      }

      currentBody = bodyFirstLine;
    } else {
      // Continuation line — append to current body
      if (currentBody !== null) {
        // Strip leading space escape for lines that look like message prefixes (spec 5.3 rule 5)
        const content = line.startsWith(" [L") ? line.slice(1) : line;
        currentBody += "\n" + content;
      }
    }
  }

  // Finalize the last entry
  finalizeEntry(entries, currentBody);

  return { entries };
}

function finalizeEntry(
  entries: ThreadEntry[],
  body: string | null,
): void {
  if (body === null || entries.length === 0) return;

  const entry = entries[entries.length - 1];
  if (entry.type === "message") {
    entry.body = body;
  } else {
    try {
      entry.meta = JSON.parse(body) as Record<string, unknown>;
    } catch {
      entry.meta = {};
    }
  }
}
