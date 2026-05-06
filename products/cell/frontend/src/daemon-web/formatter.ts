// TS port of gitim-core/src/formatter.rs — formats messages and events into .thread line format.

const MSG_PREFIX_RE = /^\[L\d{6,}\]/;

function padNumber(n: number): string {
  const s = String(n);
  return s.length >= 6 ? s : s.padStart(6, "0");
}

export function formatMessage(
  lineNumber: number,
  pointTo: number,
  author: string,
  timestamp: string,
  body: string,
): string {
  const ln = padNumber(lineNumber);
  const pt = padNumber(pointTo);
  const prefix = `[L${ln}][P${pt}][@${author}][${timestamp}] `;

  const lines = body.split("\n");
  if (lines.length === 0) {
    return prefix + "\n";
  }

  let output = prefix + lines[0] + "\n";

  for (let i = 1; i < lines.length; i++) {
    const line = lines[i];
    // Escape continuation lines that look like message prefixes
    if (MSG_PREFIX_RE.test(line)) {
      output += " ";
    }
    output += line + "\n";
  }

  return output;
}

export function formatEvent(
  lineNumber: number,
  author: string,
  timestamp: string,
  eventType: string,
  meta: Record<string, unknown>,
): string {
  const ln = padNumber(lineNumber);
  const pt = padNumber(0);
  const metaJson = JSON.stringify(meta);
  return `[L${ln}][P${pt}][@${author}][${timestamp}][E:${eventType}] ${metaJson}\n`;
}
