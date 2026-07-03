import { create } from "zustand";
import type {
  QuickSessionListItem,
  QuickSessionDetail,
  QuickSessionStatus,
} from "@/lib/types";
import {
  listQuickSessions,
  createQuickSession,
  readQuickSession,
  sendQuickSessionMessage,
  setQuickSessionTitle,
  archiveQuickSession,
} from "@/lib/client";

export interface QuickSessionWithDetail {
  item: QuickSessionListItem;
  detail?: QuickSessionDetail;
  detailLoading: boolean;
  detailError?: string;
}

interface QuickSessionState {
  sessions: QuickSessionWithDetail[];
  loading: boolean;
  error?: string;
  selectedId: string | null;

  /** Poll for active sessions. Returns true if data changed. */
  refresh(workspaceSlug: string, includeArchived?: boolean): Promise<boolean>;

  /** Create a new quick session. */
  create(
    workspaceSlug: string,
    agentId: string,
    firstMessage: string,
  ): Promise<string | null>;

  /** Load session detail (meta + thread). */
  loadDetail(workspaceSlug: string, sessionId: string): Promise<void>;

  /** Send a message to a session. */
  sendMessage(
    workspaceSlug: string,
    sessionId: string,
    body: string,
  ): Promise<boolean>;

  /** Set session title. */
  setTitle(
    workspaceSlug: string,
    sessionId: string,
    title: string,
  ): Promise<boolean>;

  /** Archive a session. */
  archive(workspaceSlug: string, sessionId: string): Promise<boolean>;

  /** Update local session status from SSE event. */
  updateStatus(sessionId: string, status: QuickSessionStatus): void;

  select(sessionId: string | null): void;
}

export const useQuickSessionStore = create<QuickSessionState>((set, get) => ({
  sessions: [],
  loading: false,
  error: undefined,
  selectedId: null,

  refresh: async (workspaceSlug: string, includeArchived?: boolean): Promise<boolean> => {
    set({ loading: true, error: undefined });
    try {
      const res = await listQuickSessions(workspaceSlug, includeArchived ?? false);
      if (!res.ok) {
        set({ loading: false, error: res.error ?? "Failed to list sessions" });
        return false;
      }
      const items = res.data ?? [];
      const current = get().sessions;
      // Preserve loaded details across refreshes
      const merged: QuickSessionWithDetail[] = items.map((item) => {
        const existing = current.find((s) => s.item.id === item.id);
        return {
          item,
          detail: existing?.detail,
          detailLoading: existing?.detailLoading ?? false,
          detailError: existing?.detailError,
        };
      });
      // De-select if selected session is now archived/deleted
      const selectedId = get().selectedId;
      const stillExists = items.some((s) => s.id === selectedId);
      set({
        sessions: merged,
        loading: false,
        selectedId: stillExists ? selectedId : (merged[0]?.item.id ?? null),
      });
      // Refresh open detail if a session is currently selected
      const newSelectedId = stillExists ? selectedId : merged[0]?.item.id;
      if (newSelectedId) {
        // Fire-and-forget: reload detail in background without blocking the list refresh
        get().loadDetail(workspaceSlug, newSelectedId);
      }
      return true;
    } catch (err) {
      set({ loading: false, error: String(err) });
      return false;
    }
  },

  create: async (
    workspaceSlug: string,
    agentId: string,
    firstMessage: string,
  ): Promise<string | null> => {
    set({ error: undefined });
    try {
      const res = await createQuickSession(workspaceSlug, agentId, firstMessage);
      if (!res.ok || !res.data) {
        set({ error: res.error ?? "Failed to create session" });
        return null;
      }
      const item = res.data;
      const session: QuickSessionWithDetail = {
        item,
        detailLoading: false,
      };
      set((s) => ({
        sessions: [session, ...s.sessions],
        selectedId: item.id,
      }));
      return item.id;
    } catch (err) {
      set({ error: String(err) });
      return null;
    }
  },

  loadDetail: async (workspaceSlug: string, sessionId: string) => {
    set((s) => ({
      sessions: s.sessions.map((entry) =>
        entry.item.id === sessionId ? { ...entry, detailLoading: true, detailError: undefined } : entry,
      ),
    }));
    try {
      const res = await readQuickSession(workspaceSlug, sessionId);
      if (!res.ok || !res.data) {
        set((s) => ({
          sessions: s.sessions.map((entry) =>
            entry.item.id === sessionId
              ? { ...entry, detailLoading: false, detailError: res.error ?? "Failed to load" }
              : entry,
          ),
        }));
        return;
      }
      set((s) => ({
        sessions: s.sessions.map((entry) =>
          entry.item.id === sessionId
            ? { ...entry, detail: res.data, detailLoading: false }
            : entry,
        ),
      }));
    } catch (err) {
      set((s) => ({
        sessions: s.sessions.map((entry) =>
          entry.item.id === sessionId
            ? { ...entry, detailLoading: false, detailError: String(err) }
            : entry,
        ),
      }));
    }
  },

  sendMessage: async (
    workspaceSlug: string,
    sessionId: string,
    body: string,
  ): Promise<boolean> => {
    try {
      const res = await sendQuickSessionMessage(workspaceSlug, sessionId, body);
      if (!res.ok) return false;
      // Reload detail to get updated thread
      await get().loadDetail(workspaceSlug, sessionId);
      return true;
    } catch {
      return false;
    }
  },

  setTitle: async (
    workspaceSlug: string,
    sessionId: string,
    title: string,
  ): Promise<boolean> => {
    try {
      const res = await setQuickSessionTitle(workspaceSlug, sessionId, title);
      return res.ok;
    } catch {
      return false;
    }
  },

  archive: async (
    workspaceSlug: string,
    sessionId: string,
  ): Promise<boolean> => {
    try {
      const res = await archiveQuickSession(workspaceSlug, sessionId);
      if (!res.ok) return false;
      set((s) => ({
        sessions: s.sessions.map((entry) =>
          entry.item.id === sessionId
            ? { ...entry, item: { ...entry.item, status: "archived" } }
            : entry,
        ),
      }));
      return true;
    } catch {
      return false;
    }
  },

  updateStatus: (sessionId: string, status: QuickSessionStatus) => {
    set((s) => ({
      sessions: s.sessions.map((entry) =>
        entry.item.id === sessionId
          ? { ...entry, item: { ...entry.item, status } }
          : entry,
      ),
    }));
  },

  select: (sessionId: string | null) => set({ selectedId: sessionId }),
}));

/** Short display ID: #<last-6-chars> */
export function shortSessionId(id: string): string {
  return `#${id.slice(-6)}`;
}

/** Format thread raw text into structured messages for display. */
export interface ThreadMessage {
  author: string;
  body: string;
  line: string;
}

const THREAD_MESSAGE_RE =
  /^\[L(\d{6,})\]\[P(\d{6,})\]\[@([a-z0-9-]+)\]\[(\d{8}T\d{6}Z)\](?:\[E:([a-z][a-z0-9_-]*)\])? (.*)$/;

export function parseThread(raw: string): ThreadMessage[] {
  const messages: ThreadMessage[] = [];
  let current: ThreadMessage | null = null;

  const lines = raw.replace(/\r\n/g, "\n").split("\n");
  for (let index = 0; index < lines.length; index += 1) {
    const line = lines[index] ?? "";
    if (line === "") {
      if (index === lines.length - 1) continue;
      if (current) current.body += "\n";
      continue;
    }
    const match = line.match(THREAD_MESSAGE_RE);
    if (match) {
      if (current) messages.push(current);
      const eventType = match[5];
      current = eventType
        ? null
        : {
            line: `L${match[1]}`,
            author: match[3],
            body: match[6],
          };
      continue;
    }
    if (current) {
      current.body += `\n${line.startsWith(" [L") ? line.slice(1) : line}`;
    }
  }

  if (current) messages.push(current);
  return messages;
}
