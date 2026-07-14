import { create } from "zustand";

import * as client from "../lib/client";
import type {
  AgentActivityEvent,
  PollChange,
  QuickSessionDetail,
  QuickSessionListItem,
  QuickSessionStatus,
  SessionUsageSnapshot,
} from "../lib/types";
import { onWorkspaceSwitch } from "../lib/workspace-lifecycle";

type QuickSessionOperation =
  | "list"
  | "detail"
  | "create"
  | "send"
  | "archive";

type OperationFlags = Record<QuickSessionOperation, boolean>;
type OperationErrors = Record<QuickSessionOperation, string | null>;

export interface QuickSessionRuntimeOverlay {
  status: QuickSessionStatus | AgentActivityEvent["event_type"];
  revision: number;
  attemptId?: string;
  contextGeneration?: number;
  latestEvent?: AgentActivityEvent;
  usage?: SessionUsageSnapshot;
}

interface QuickSessionState {
  activeSlug: string | null;
  items: QuickSessionListItem[];
  selectedId: string | null;
  detailById: Record<string, QuickSessionDetail>;
  runtimeById: Record<string, QuickSessionRuntimeOverlay>;
  showArchived: boolean;
  loading: OperationFlags;
  errors: OperationErrors;

  setShowArchived: (showArchived: boolean) => void;
  select: (id: string | null) => void;
  refreshList: (slug: string) => Promise<void>;
  open: (slug: string, id: string) => Promise<void>;
  create: (slug: string, agentId: string, firstMessage: string) => Promise<string | null>;
  send: (slug: string, id: string, body: string) => Promise<boolean>;
  archive: (slug: string, id: string) => Promise<boolean>;
  unarchive: (slug: string, id: string) => Promise<boolean>;
  refreshFromPoll: (slug: string, changes: PollChange[]) => Promise<void>;
  applyDetail: (detail: QuickSessionDetail) => boolean;
  applyActivityEvent: (event: AgentActivityEvent) => boolean;
  resetForWorkspaceSwitch: () => void;
}

const EMPTY_LOADING: OperationFlags = {
  list: false,
  detail: false,
  create: false,
  send: false,
  archive: false,
};

const EMPTY_ERRORS: OperationErrors = {
  list: null,
  detail: null,
  create: null,
  send: null,
  archive: null,
};

let workspaceGeneration = 0;
const operationSequence: Record<QuickSessionOperation, number> = {
  list: 0,
  detail: 0,
  create: 0,
  send: 0,
  archive: 0,
};

function beginOperation(operation: QuickSessionOperation): {
  generation: number;
  sequence: number;
} {
  operationSequence[operation] += 1;
  return {
    generation: workspaceGeneration,
    sequence: operationSequence[operation],
  };
}

function operationIsCurrent(
  slug: string,
  operation: QuickSessionOperation,
  request: { generation: number; sequence: number },
): boolean {
  return (
    request.generation === workspaceGeneration &&
    request.sequence === operationSequence[operation] &&
    useQuickSessionStore.getState().activeSlug === slug
  );
}

function activateSlug(slug: string): void {
  const store = useQuickSessionStore.getState();
  if (store.activeSlug === slug) return;
  if (store.activeSlug !== null) store.resetForWorkspaceSwitch();
  useQuickSessionStore.setState({ activeSlug: slug });
}

function itemFromDetail(detail: QuickSessionDetail): QuickSessionListItem {
  return {
    id: detail.meta.id,
    title: detail.meta.title ?? null,
    agent_id: detail.meta.agent_id,
    created_by: detail.meta.created_by,
    status: detail.meta.status,
    updated_at: detail.meta.updated_at,
    last_message_preview: detail.meta.last_message_preview,
    revision: detail.meta.revision,
    archived: detail.archived,
    ref: `session:${detail.meta.id}`,
  };
}

function mapQuickSessionUsage(detail: string): SessionUsageSnapshot | undefined {
  if (detail.trim() === "") return undefined;
  try {
    const raw = JSON.parse(detail) as Record<string, unknown>;
    return {
      sessionId: (raw.session_id as string) ?? "",
      inputTokens: raw.input_tokens as number | undefined,
      outputTokens: raw.output_tokens as number | undefined,
      maxTokens: raw.max_tokens as number | undefined,
      usedPercent: (raw.used_percent as number) ?? 0,
      source:
        (raw.source as SessionUsageSnapshot["source"]) ??
        "provider_reported",
      updatedAt: (raw.updated_at as string) ?? "",
    };
  } catch {
    return undefined;
  }
}

function setOperation(
  set: (partial: Partial<QuickSessionState>) => void,
  operation: QuickSessionOperation,
  loading: boolean,
  error: string | null,
): void {
  const state = useQuickSessionStore.getState();
  set({
    loading: { ...state.loading, [operation]: loading },
    errors: { ...state.errors, [operation]: error },
  });
}

export const useQuickSessionStore = create<QuickSessionState>((set, get) => ({
  activeSlug: null,
  items: [],
  selectedId: null,
  detailById: {},
  runtimeById: {},
  showArchived: false,
  loading: { ...EMPTY_LOADING },
  errors: { ...EMPTY_ERRORS },

  setShowArchived: (showArchived) => set({ showArchived }),
  select: (selectedId) => set({ selectedId }),

  refreshList: async (slug) => {
    activateSlug(slug);
    const request = beginOperation("list");
    setOperation(set, "list", true, null);
    const response = await client.listQuickSessions(slug, {
      archived: get().showArchived,
    });
    if (!operationIsCurrent(slug, "list", request)) return;
    if (!response.ok || !response.data) {
      setOperation(
        set,
        "list",
        false,
        response.error ?? "Failed to list Quick Sessions",
      );
      return;
    }
    set((state) => {
      const existing = new Map(state.items.map((item) => [item.id, item]));
      const items = response.data!.sessions.map((item) => {
        const current = existing.get(item.id);
        return current && current.revision > item.revision ? current : item;
      });
      return {
        items,
        loading: { ...state.loading, list: false },
        errors: { ...state.errors, list: null },
      };
    });
  },

  open: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("detail");
    set((state) => ({
      selectedId: id,
      loading: { ...state.loading, detail: true },
      errors: { ...state.errors, detail: null },
    }));
    const response = await client.readQuickSession(slug, id);
    if (!operationIsCurrent(slug, "detail", request)) return;
    if (!response.ok || !response.data) {
      setOperation(
        set,
        "detail",
        false,
        response.error ?? "Failed to read Quick Session",
      );
      return;
    }
    get().applyDetail(response.data.session);
    setOperation(set, "detail", false, null);
  },

  create: async (slug, agentId, firstMessage) => {
    activateSlug(slug);
    const request = beginOperation("create");
    setOperation(set, "create", true, null);
    const response = await client.createQuickSession(slug, agentId, firstMessage);
    if (!operationIsCurrent(slug, "create", request)) return null;
    if (!response.ok || !response.data) {
      setOperation(
        set,
        "create",
        false,
        response.error ?? "Failed to create Quick Session",
      );
      return null;
    }
    get().applyDetail(response.data.session);
    set({ selectedId: response.data.session.meta.id });
    setOperation(set, "create", false, null);
    return response.data.session.meta.id;
  },

  send: async (slug, id, body) => {
    activateSlug(slug);
    const request = beginOperation("send");
    setOperation(set, "send", true, null);
    const response = await client.sendQuickSessionMessage(slug, id, body);
    if (!operationIsCurrent(slug, "send", request)) return false;
    if (!response.ok) {
      setOperation(
        set,
        "send",
        false,
        response.error ?? "Failed to send Quick Session message",
      );
      return false;
    }
    await get().open(slug, id);
    if (!operationIsCurrent(slug, "send", request)) return false;
    setOperation(set, "send", false, null);
    return true;
  },

  archive: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("archive");
    setOperation(set, "archive", true, null);
    const response = await client.archiveQuickSession(slug, id);
    if (!operationIsCurrent(slug, "archive", request)) return false;
    if (!response.ok) {
      setOperation(
        set,
        "archive",
        false,
        response.error ?? "Failed to archive Quick Session",
      );
      return false;
    }
    await get().refreshList(slug);
    if (!operationIsCurrent(slug, "archive", request)) return false;
    setOperation(set, "archive", false, null);
    return true;
  },

  unarchive: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("archive");
    setOperation(set, "archive", true, null);
    const response = await client.unarchiveQuickSession(slug, id);
    if (!operationIsCurrent(slug, "archive", request)) return false;
    if (!response.ok) {
      setOperation(
        set,
        "archive",
        false,
        response.error ?? "Failed to unarchive Quick Session",
      );
      return false;
    }
    await get().refreshList(slug);
    if (!operationIsCurrent(slug, "archive", request)) return false;
    setOperation(set, "archive", false, null);
    return true;
  },

  refreshFromPoll: async (slug, changes) => {
    activateSlug(slug);
    const sessionIds = new Set(
      changes
        .filter(
          (change) =>
            change.kind === "quick_session_meta" ||
            change.kind === "quick_session_thread",
        )
        .map((change) => change.channel),
    );
    if (sessionIds.size === 0) return;
    const selectedId = get().selectedId;
    await Promise.all([
      get().refreshList(slug),
      selectedId && sessionIds.has(selectedId)
        ? get().open(slug, selectedId)
        : Promise.resolve(),
    ]);
  },

  applyDetail: (detail) => {
    const id = detail.meta.id;
    const current = get().detailById[id];
    if (current && current.meta.revision > detail.meta.revision) return false;
    set((state) => {
      const incoming = itemFromDetail(detail);
      const without = state.items.filter((item) => item.id !== id);
      const items =
        detail.archived === state.showArchived ? [...without, incoming] : without;
      const previousRuntime = state.runtimeById[id];
      const sameAttempt =
        detail.meta.attempt_id !== undefined &&
        detail.meta.attempt_id === previousRuntime?.attemptId;
      const runtime: QuickSessionRuntimeOverlay = sameAttempt
        ? {
            ...previousRuntime,
            status: previousRuntime.status,
            revision: detail.meta.revision,
          }
        : {
            status: detail.meta.status,
            revision: detail.meta.revision,
            ...(detail.meta.attempt_id
              ? { attemptId: detail.meta.attempt_id }
              : {}),
            ...(previousRuntime?.usage ? { usage: previousRuntime.usage } : {}),
          };
      return {
        items,
        detailById: { ...state.detailById, [id]: detail },
        runtimeById: { ...state.runtimeById, [id]: runtime },
      };
    });
    return true;
  },

  applyActivityEvent: (event) => {
    if (
      event.scope !== "quick_session" ||
      !event.session_id ||
      !event.attempt_id ||
      event.context_generation === undefined ||
      event.session_revision === undefined
    ) {
      return false;
    }
    const detail = get().detailById[event.session_id];
    if (
      !detail ||
      detail.meta.status !== "running" ||
      detail.meta.attempt_id !== event.attempt_id
    ) {
      return false;
    }
    const previous = get().runtimeById[event.session_id];
    if (
      previous?.attemptId === event.attempt_id &&
      previous.contextGeneration !== undefined &&
      previous.contextGeneration !== event.context_generation
    ) {
      return false;
    }
    set((state) => {
      const current = state.runtimeById[event.session_id!];
      const usage =
        event.event_type === "usage"
          ? mapQuickSessionUsage(event.detail)
          : current?.usage;
      return {
        runtimeById: {
          ...state.runtimeById,
          [event.session_id!]: {
            status: event.event_type,
            revision: Math.max(
              detail.meta.revision,
              current?.revision ?? 0,
              event.session_revision!,
            ),
            attemptId: event.attempt_id,
            contextGeneration: event.context_generation,
            latestEvent: event,
            ...(usage ? { usage } : {}),
          },
        },
      };
    });
    return true;
  },

  resetForWorkspaceSwitch: () => {
    workspaceGeneration += 1;
    for (const operation of Object.keys(operationSequence) as QuickSessionOperation[]) {
      operationSequence[operation] += 1;
    }
    set({
      activeSlug: null,
      items: [],
      selectedId: null,
      detailById: {},
      runtimeById: {},
      showArchived: false,
      loading: { ...EMPTY_LOADING },
      errors: { ...EMPTY_ERRORS },
    });
  },
}));

onWorkspaceSwitch(() => {
  useQuickSessionStore.getState().resetForWorkspaceSwitch();
});
