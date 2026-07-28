import { create } from "zustand";

/**
 * Composer operation tokens — the single source of truth for the question
 * "an async composer operation finished after the world changed; are its
 * late effects still valid?"
 *
 * One operation per composer scope (an attachmentDraftKey) may be active at a
 * time. beginOperation mints a fresh, never-reused token and marks the scope
 * busy; it refuses (null) while another operation is active, so a newer begin
 * can never invalidate a live token. A token dies only when its operation
 * finishes (settleOperation / endOperation) or when its scope is invalidated
 * (invalidateWorkspace / invalidateAll). Every late effect of an async
 * operation — attachment write-backs in use-attachment-draft-store, and text
 * clearing / reply clearing / busy release in InputArea — is admitted through
 * this store: synchronously via isOperationCurrent for in-flight write-backs,
 * or via settleOperation for successful settlement.
 *
 * settleOperation CAS-validates the token, releases the scope, and publishes
 * a completion event carrying the token plus the captured text/reply
 * fingerprint — atomically, so a stale success can neither publish nor free a
 * newer operation's scope. Mounted consumers apply an event only when its
 * scope key matches their current scope and its fingerprint still matches
 * their current UI state (the operation itself has already settled by then,
 * so the fingerprint — not token currency — is the consumption guard).
 * Events are kept in a small ring so subscribers mounted or remounted after
 * publication can still observe them; the ring is append-only and survives
 * scope disposal exactly like the completion stream it replaces.
 */

export type ComposerOperationKind = "attachments" | "text";

export interface ComposerOperation {
  readonly token: number;
  readonly kind: ComposerOperationKind;
}

export interface ComposerCompletion {
  readonly sequence: number;
  readonly key: string;
  readonly token: number;
  readonly text: string;
  readonly replyLine: number | null;
}

export interface ComposerSettlement {
  readonly text: string;
  readonly replyLine: number | null;
}

interface ComposerOperationStore {
  readonly operations: Readonly<Record<string, ComposerOperation>>;
  readonly completionSequence: number;
  readonly completions: readonly ComposerCompletion[];
  /**
   * Mint a token and mark the scope busy with an operation of `kind`.
   * Returns null when the scope already has an active operation — a scope
   * runs at most one composer operation at a time, and an active operation
   * is never replaced. The busy check runs inside the store update, so a
   * subscriber racing the mint cannot make two callers both win.
   */
  beginOperation: (key: string, kind: ComposerOperationKind) => ComposerOperation | null;
  /** The unified stale-effect check: true iff (key, token) is the scope's active operation. */
  isOperationCurrent: (key: string, token: number) => boolean;
  /**
   * Release the scope's busy mark iff (key, token) is still current. The
   * token is re-validated inside the store update (compare-and-swap), and
   * the boolean result is the caller's gate for every other late effect.
   */
  endOperation: (key: string, token: number) => boolean;
  /**
   * Settle a successful operation atomically: CAS-validate (key, token),
   * release the scope, and publish the completion event in the same store
   * update. Returns false — publishing nothing and touching no newer
   * operation — when the token is no longer current (disposed or
   * invalidated while the request was in flight), so callers can skip
   * every other late effect of the stale path.
   */
  settleOperation: (key: string, token: number, settlement: ComposerSettlement) => boolean;
  /**
   * Invalidate every active operation whose scope key encodes `workspaceKey`
   * — regardless of whether the scope has an attachment draft, so text-only
   * operations are invalidated by workspace disposal too.
   */
  invalidateWorkspace: (workspaceKey: string) => void;
  /** Invalidate every active operation (full draft disposal). */
  invalidateAll: () => void;
}

const MAX_COMPLETION_EVENTS = 32;
const EMPTY_OPERATIONS: Readonly<Record<string, ComposerOperation>> = Object.freeze({});

/**
 * Derive the workspace segment from an encoded composer scope key — the
 * inverse of attachmentDraftKey's two-element tuple encoding.
 */
export function scopeWorkspaceKey(key: string): string | undefined {
  try {
    const tuple: unknown = JSON.parse(key);
    return Array.isArray(tuple) && tuple.length === 2 && typeof tuple[0] === "string"
      ? tuple[0]
      : undefined;
  } catch {
    return undefined;
  }
}

function replaceOperation(
  operations: Readonly<Record<string, ComposerOperation>>,
  key: string,
  operation?: ComposerOperation,
): Readonly<Record<string, ComposerOperation>> {
  const next = { ...operations };
  if (operation === undefined) {
    delete next[key];
  } else {
    next[key] = operation;
  }
  return Object.freeze(next);
}

export const useComposerOperationStore = create<ComposerOperationStore>((set, get) => {
  let nextToken = 1;

  return {
    operations: EMPTY_OPERATIONS,
    completionSequence: 0,
    completions: [],

    beginOperation: (key, kind) => {
      // Mint inside the update so the busy check and the write are atomic
      // with respect to subscriber callbacks; a busy scope returns the state
      // unchanged (no notification) and yields null.
      let minted: ComposerOperation | null = null;
      set((state) => {
        if (state.operations[key] !== undefined) return state;
        minted = Object.freeze({ token: nextToken++, kind });
        return { operations: replaceOperation(state.operations, key, minted) };
      });
      return minted;
    },

    isOperationCurrent: (key, token) => get().operations[key]?.token === token,

    endOperation: (key, token) => {
      let ended = false;
      set((state) => {
        if (state.operations[key]?.token !== token) return state;
        ended = true;
        return { operations: replaceOperation(state.operations, key) };
      });
      return ended;
    },

    settleOperation: (key, token, settlement) => {
      let settled = false;
      set((state) => {
        if (state.operations[key]?.token !== token) return state;
        settled = true;
        const sequence = state.completionSequence + 1;
        return {
          operations: replaceOperation(state.operations, key),
          completionSequence: sequence,
          completions: [
            ...state.completions,
            Object.freeze({
              sequence,
              key,
              token,
              text: settlement.text,
              replyLine: settlement.replyLine,
            }),
          ].slice(-MAX_COMPLETION_EVENTS),
        };
      });
      return settled;
    },

    invalidateWorkspace: (workspaceKey) => {
      const active = Object.keys(get().operations).filter(
        (key) => scopeWorkspaceKey(key) === workspaceKey,
      );
      if (active.length === 0) return;
      set((state) => {
        const next = { ...state.operations };
        for (const key of active) {
          delete next[key];
        }
        return { operations: Object.freeze(next) };
      });
    },

    invalidateAll: () => {
      if (Object.keys(get().operations).length === 0) return;
      set({ operations: EMPTY_OPERATIONS });
    },
  };
});
