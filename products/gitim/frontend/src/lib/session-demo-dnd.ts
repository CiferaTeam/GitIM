export const SESSION_DEMO_MIME = "application/x-gitim-session-demo";

export interface SessionDemoDragPayload {
  id: string;
  title: string;
  agent: string;
  ref: string;
}

export function dataTransferHasSessionDemo(
  types: DOMStringList | readonly string[],
): boolean {
  if ("contains" in types) {
    return types.contains(SESSION_DEMO_MIME);
  }
  return types.includes(SESSION_DEMO_MIME);
}

export function readSessionDemoDragPayload(
  dataTransfer: DataTransfer,
): SessionDemoDragPayload | null {
  const raw = dataTransfer.getData(SESSION_DEMO_MIME);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Partial<SessionDemoDragPayload>;
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
