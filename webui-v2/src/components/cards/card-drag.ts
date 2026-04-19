export const CARD_DRAG_MIME = "application/x-gitim-card";

export interface CardDragPayload {
  channel: string;
  card_id: string;
}

export function encodeCardDrag(p: CardDragPayload): string {
  return JSON.stringify(p);
}

export function decodeCardDrag(raw: string): CardDragPayload | null {
  try {
    const parsed = JSON.parse(raw) as Partial<CardDragPayload>;
    if (
      typeof parsed.channel === "string" &&
      typeof parsed.card_id === "string"
    ) {
      return { channel: parsed.channel, card_id: parsed.card_id };
    }
    return null;
  } catch {
    return null;
  }
}
