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
  /** Archived cards — flat list across channels; selectors can filter per-channel. */
  archivedCards: Card[];
  /** UI toggle: whether the Archived view is visible in CardsPanel. */
  showArchived: boolean;
  /** Discussion messages keyed by "<channel>/<card_id>". */
  cardMessagesByPath: Record<string, Message[]>;
  /** Paths of cards with an in-flight PATCH; skip poll-driven overwrites for these. */
  inFlightCardPaths: Set<string>;

  setCards: (cards: Card[]) => void;
  /** Merge incoming cards from server, preserving any with in-flight patches. */
  mergeCards: (cards: Card[]) => void;
  upsertCard: (card: Card) => void;
  removeCard: (channel: string, cardId: string) => void;
  markCardInFlight: (channel: string, cardId: string) => void;
  unmarkCardInFlight: (channel: string, cardId: string) => void;

  setArchivedCards: (cards: Card[]) => void;
  /** Add-or-replace an archived card by (channel, card_id). */
  upsertArchivedCard: (card: Card) => void;
  toggleShowArchived: () => void;
  /** Optimistic: move a card from `cards` → `archivedCards`. */
  markArchived: (channel: string, cardId: string) => void;
  /** Optimistic: move a card from `archivedCards` → `cards`. */
  markUnarchived: (channel: string, cardId: string) => void;

  setCardMessages: (pathKey: string, messages: Message[]) => void;
  addCardMessages: (pathKey: string, messages: Message[]) => void;
  addPendingCardMessage: (pathKey: string, msg: Message) => void;
  markPendingCardSent: (
    pathKey: string,
    pendingId: string,
    lineNumber: number,
  ) => void;
  markPendingCardFailed: (
    pathKey: string,
    pendingId: string,
    lineNumber?: number,
  ) => void;
  removePendingCardMessage: (pathKey: string, pendingId: string) => void;
  /** Clear all workspace-scoped card state. Called on workspace switch. */
  resetForWorkspaceSwitch: () => void;
}

export const useCardStore = create<CardState>((set) => ({
  cards: [],
  archivedCards: [],
  showArchived: false,
  cardMessagesByPath: {},
  inFlightCardPaths: new Set<string>(),

  setCards: (cards) => set({ cards }),

  mergeCards: (incoming) =>
    set((state) => {
      // Replace each card by (channel, card_id) but keep local version
      // whenever a PATCH is in flight for that card — otherwise the 3s
      // poll cadence clobbers optimistic UI until the next tick.
      const byKey = new Map<string, Card>();
      for (const c of state.cards) {
        byKey.set(cardPathKey(c.channel, c.card_id), c);
      }
      for (const c of incoming) {
        const k = cardPathKey(c.channel, c.card_id);
        if (state.inFlightCardPaths.has(k)) continue;
        byKey.set(k, c);
      }
      // Also drop any local cards that disappeared server-side, EXCEPT in-flight ones
      // (those are user-created and not yet committed, or being edited).
      const serverKeys = new Set(
        incoming.map((c) => cardPathKey(c.channel, c.card_id)),
      );
      const next: Card[] = [];
      for (const [k, c] of byKey) {
        if (serverKeys.has(k) || state.inFlightCardPaths.has(k)) {
          next.push(c);
        }
      }
      return { cards: next };
    }),

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

  markCardInFlight: (channel, cardId) =>
    set((state) => {
      const next = new Set(state.inFlightCardPaths);
      next.add(cardPathKey(channel, cardId));
      return { inFlightCardPaths: next };
    }),

  unmarkCardInFlight: (channel, cardId) =>
    set((state) => {
      const next = new Set(state.inFlightCardPaths);
      next.delete(cardPathKey(channel, cardId));
      return { inFlightCardPaths: next };
    }),

  setArchivedCards: (cards) => set({ archivedCards: cards }),

  upsertArchivedCard: (card) =>
    set((state) => {
      const idx = state.archivedCards.findIndex(
        (c) => c.channel === card.channel && c.card_id === card.card_id,
      );
      if (idx === -1) {
        return { archivedCards: [...state.archivedCards, card] };
      }
      const next = state.archivedCards.slice();
      next[idx] = card;
      return { archivedCards: next };
    }),

  toggleShowArchived: () =>
    set((state) => ({ showArchived: !state.showArchived })),

  markArchived: (channel, cardId) =>
    set((state) => {
      const card = state.cards.find(
        (c) => c.channel === channel && c.card_id === cardId,
      );
      const nextCards = state.cards.filter(
        (c) => !(c.channel === channel && c.card_id === cardId),
      );
      // Only push to archived when we actually had the card locally, AND
      // when it's not already in archivedCards (dedup by channel + card_id).
      const alreadyArchived = state.archivedCards.some(
        (c) => c.channel === channel && c.card_id === cardId,
      );
      const nextArchived =
        card && !alreadyArchived
          ? [...state.archivedCards, card]
          : state.archivedCards;
      return { cards: nextCards, archivedCards: nextArchived };
    }),

  markUnarchived: (channel, cardId) =>
    set((state) => {
      const card = state.archivedCards.find(
        (c) => c.channel === channel && c.card_id === cardId,
      );
      const nextArchived = state.archivedCards.filter(
        (c) => !(c.channel === channel && c.card_id === cardId),
      );
      const alreadyActive = state.cards.some(
        (c) => c.channel === channel && c.card_id === cardId,
      );
      const nextCards =
        card && !alreadyActive ? [...state.cards, card] : state.cards;
      return { cards: nextCards, archivedCards: nextArchived };
    }),

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
      const realAlreadyArrived = existing.some(
        (m) => !m._pendingId && m.line_number === lineNumber,
      );
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: realAlreadyArrived
            ? existing.filter((m) => m._pendingId !== pendingId)
            : existing.map((m) =>
                m._pendingId === pendingId
                  ? { ...m, _status: "sent", line_number: lineNumber }
                  : m,
              ),
        },
      };
    }),

  markPendingCardFailed: (pathKey, pendingId, lineNumber) =>
    set((state) => {
      const existing = state.cardMessagesByPath[pathKey] ?? [];
      return {
        cardMessagesByPath: {
          ...state.cardMessagesByPath,
          [pathKey]: existing.map((m) =>
            m._pendingId === pendingId
              ? {
                  ...m,
                  _status: "failed",
                  ...(lineNumber !== undefined && { line_number: lineNumber }),
                }
              : m,
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

  resetForWorkspaceSwitch: () =>
    set({
      cards: [],
      archivedCards: [],
      showArchived: false,
      cardMessagesByPath: {},
      inFlightCardPaths: new Set<string>(),
    }),
}));

// ─── Derived selectors (call as regular functions with state) ───────────────

// Memoized by `state.cards` identity so zustand's Object.is check over the
// selector result stays stable — otherwise useSyncExternalStore's getSnapshot
// would return a fresh array every render and loop forever.
let _labelsCardsRef: Card[] | null = null;
let _labelsCache: string[] = [];
export function selectAllLabels(state: CardState): string[] {
  if (state.cards === _labelsCardsRef) return _labelsCache;
  const set = new Set<string>();
  for (const card of state.cards) {
    for (const l of card.labels) set.add(l);
  }
  _labelsCardsRef = state.cards;
  _labelsCache = [...set].sort();
  return _labelsCache;
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
