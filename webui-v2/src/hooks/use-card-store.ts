import { create } from "zustand";
import type { Card, CardFilter, Message } from "../lib/types";

/** Key used to index discussion messages per card: "<channel>/<card_id>". */
export function cardPathKey(channel: string, cardId: string): string {
  return `${channel}/${cardId}`;
}

/** Scope key used for InputArea / MessageList when rendering a card discussion. */
export function cardScopeKey(channel: string, cardId: string): string {
  return `card:${channel}/${cardId}`;
}

/** Parse a scope channel string of the form "card:<channel>/<card_id>" back into parts. */
export function parseCardScope(channelStr: string): {
  channel: string;
  cardId: string;
} | null {
  if (!channelStr.startsWith("card:")) return null;
  const rest = channelStr.slice("card:".length);
  const idx = rest.indexOf("/");
  if (idx < 0) return null;
  return { channel: rest.slice(0, idx), cardId: rest.slice(idx + 1) };
}

interface CardState {
  cards: Card[];
  /** Discussion messages keyed by "<channel>/<card_id>". */
  cardMessagesByPath: Record<string, Message[]>;

  setCards: (cards: Card[]) => void;
  upsertCard: (card: Card) => void;
  removeCard: (channel: string, cardId: string) => void;

  setCardMessages: (pathKey: string, messages: Message[]) => void;
  addCardMessages: (pathKey: string, messages: Message[]) => void;
  addPendingCardMessage: (pathKey: string, msg: Message) => void;
  markPendingCardSent: (
    pathKey: string,
    pendingId: string,
    lineNumber: number,
  ) => void;
  markPendingCardFailed: (pathKey: string, pendingId: string) => void;
  removePendingCardMessage: (pathKey: string, pendingId: string) => void;
}

export const useCardStore = create<CardState>((set) => ({
  cards: [],
  cardMessagesByPath: {},

  setCards: (cards) => set({ cards }),

  upsertCard: (card) =>
    set((state) => {
      const idx = state.cards.findIndex(
        (c) => c.channel === card.channel && c.card_id === card.card_id,
      );
      if (idx === -1) {
        return { cards: [...state.cards, card] };
      }
      const next = state.cards.slice();
      next[idx] = card;
      return { cards: next };
    }),

  removeCard: (channel, cardId) =>
    set((state) => ({
      cards: state.cards.filter(
        (c) => !(c.channel === channel && c.card_id === cardId),
      ),
    })),

  setCardMessages: (pathKey, messages) =>
    set((state) => ({
      cardMessagesByPath: { ...state.cardMessagesByPath, [pathKey]: messages },
    })),

  addCardMessages: (pathKey, incoming) =>
    set((state) => {
      const existing = state.cardMessagesByPath[pathKey] ?? [];
      const lines = new Set(existing.map((m) => m.line_number));
      const toAdd = incoming.filter((m) => !lines.has(m.line_number));
      if (toAdd.length === 0) return {};
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: [...existing, ...toAdd],
        },
      };
    }),

  addPendingCardMessage: (pathKey, msg) =>
    set((state) => ({
      cardMessagesByPath: {
        ...state.cardMessagesByPath,
        [pathKey]: [...(state.cardMessagesByPath[pathKey] ?? []), msg],
      },
    })),

  markPendingCardSent: (pathKey, pendingId, lineNumber) =>
    set((state) => {
      const existing = state.cardMessagesByPath[pathKey] ?? [];
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: existing.map((m) =>
            m._pendingId === pendingId
              ? { ...m, _status: "sent", line_number: lineNumber }
              : m,
          ),
        },
      };
    }),

  markPendingCardFailed: (pathKey, pendingId) =>
    set((state) => {
      const existing = state.cardMessagesByPath[pathKey] ?? [];
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: existing.map((m) =>
            m._pendingId === pendingId ? { ...m, _status: "failed" } : m,
          ),
        },
      };
    }),

  removePendingCardMessage: (pathKey, pendingId) =>
    set((state) => {
      const existing = state.cardMessagesByPath[pathKey] ?? [];
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: existing.filter((m) => m._pendingId !== pendingId),
        },
      };
    }),
}));

// ─── Derived selectors (call as regular functions with state) ───────────────

export function selectAllLabels(state: CardState): string[] {
  const set = new Set<string>();
  for (const card of state.cards) {
    for (const l of card.labels) set.add(l);
  }
  return [...set].sort();
}

export function selectFilteredCards(
  cards: Card[],
  filter: CardFilter,
  currentUser: string | null,
): Card[] {
  return cards.filter((card) => {
    if (filter.channels && filter.channels.length > 0) {
      if (!filter.channels.includes(card.channel)) return false;
    } else if (filter.channel) {
      if (card.channel !== filter.channel) return false;
    }
    if (filter.status && card.status !== filter.status) return false;
    if (filter.assignee) {
      const target = filter.assignee === "__me__" ? currentUser : filter.assignee;
      if (!target) return false;
      if (filter.assignee === "__unassigned__") {
        if (card.assignee) return false;
      } else if (card.assignee !== target) {
        return false;
      }
    }
    if (filter.labels && filter.labels.length > 0) {
      const cardLabels = new Set(card.labels);
      for (const l of filter.labels) {
        if (!cardLabels.has(l)) return false;
      }
    }
    return true;
  });
}

export function selectCardById(
  state: CardState,
  channel: string,
  cardId: string,
): Card | undefined {
  return state.cards.find(
    (c) => c.channel === channel && c.card_id === cardId,
  );
}

/** Sort cards by updated_at DESC (YYYYMMDDTHHMMSSZ format sorts lexicographically). */
export function sortByUpdatedDesc(cards: Card[]): Card[] {
  return [...cards].sort((a, b) => {
    if (a.updated_at === b.updated_at) return 0;
    return a.updated_at < b.updated_at ? 1 : -1;
  });
}
