import { create } from "zustand";

import * as client from "../lib/client";
import type {
  AgentActivityEvent,
  ArchiveQuickSessionResponse,
  PollChange,
  QuickSessionDetail,
  QuickSessionListItem,
  QuickSessionStatus,
} from "../lib/types";
import {
  formatQuickSessionRef,
  generateQuickSessionId,
  generateQuickSessionRequestId,
} from "../lib/quick-session-ref";
import { onWorkspaceSwitch } from "../lib/workspace-lifecycle";

type QuickSessionOperation =
  | "list"
  | "detail"
  | "create"
  | "send"
  | "archive";

type OperationFlags = Record<QuickSessionOperation, boolean>;
type OperationErrors = Record<QuickSessionOperation, string | null>;

interface PendingCreateOperation {
  slug: string;
  agentId: string;
  firstMessage: string;
  sessionId: string;
}

interface PendingSendOperation {
  slug: string;
  sessionId: string;
  body: string;
  requestId: string;
}

type ScopedQuickSessionActivity = AgentActivityEvent & {
  scope: "quick_session";
  session_id: string;
  attempt_id: string;
  context_generation: number;
  session_revision: number;
};

export interface QuickSessionRuntimeOverlay {
  status: QuickSessionStatus | AgentActivityEvent["event_type"] | "queued";
  revision: number;
  attemptId?: string;
  contextGeneration?: number;
  latestEvent?: AgentActivityEvent;
  queuedInputLine?: number;
}

interface QuickSessionState {
  activeSlug: string | null;
  items: QuickSessionListItem[];
  selectedId: string | null;
  detailById: Record<string, QuickSessionDetail>;
  runtimeById: Record<string, QuickSessionRuntimeOverlay>;
  pendingActivityById: Record<string, ScopedQuickSessionActivity>;
  showArchived: boolean;
  loading: OperationFlags;
  errors: OperationErrors;
  /** At most one retryable mutation per operation; a changed payload cancels it. */
  pendingCreate: PendingCreateOperation | null;
  pendingSend: PendingSendOperation | null;

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

const QUICK_SESSION_HUB_TRANSCRIPT_LIMIT = 50;
const MAX_PENDING_ACTIVITIES = 64;

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
    ref: formatQuickSessionRef(detail.meta.id),
  };
}

function sortQuickSessionItems(
  items: QuickSessionListItem[],
): QuickSessionListItem[] {
  return [...items].sort((left, right) => {
    const updated = right.updated_at.localeCompare(left.updated_at);
    return updated || left.id.localeCompare(right.id);
  });
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

function rejectedOperationMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message.trim() !== "") {
    return error.message;
  }
  if (typeof error === "string" && error.trim() !== "") return error;
  return fallback;
}

function sameCreatePayload(
  pending: PendingCreateOperation,
  slug: string,
  agentId: string,
  firstMessage: string,
): boolean {
  return (
    pending.slug === slug &&
    pending.agentId === agentId &&
    pending.firstMessage === firstMessage
  );
}

function sameSendPayload(
  pending: PendingSendOperation,
  slug: string,
  sessionId: string,
  body: string,
): boolean {
  return (
    pending.slug === slug &&
    pending.sessionId === sessionId &&
    pending.body === body
  );
}

function isScopedQuickSessionActivity(
  event: AgentActivityEvent,
): event is ScopedQuickSessionActivity {
  return (
    event.scope === "quick_session" &&
    event.session_id !== undefined &&
    event.session_id !== "" &&
    event.attempt_id !== undefined &&
    event.attempt_id !== "" &&
    event.context_generation !== undefined &&
    event.session_revision !== undefined
  );
}

function detailOwnsActivity(
  detail: QuickSessionDetail,
  event: ScopedQuickSessionActivity,
): boolean {
  const ownsActiveAttempt =
    detail.meta.status === "running" &&
    detail.meta.attempt_id === event.attempt_id;
  const ownsCompletedAttempt =
    detail.meta.status !== "running" &&
    detail.meta.status !== "archived" &&
    (event.event_type === "usage" || event.event_type === "done") &&
    detail.meta.last_completed_attempt_id === event.attempt_id;
  const ownsFailedAttempt =
    detail.meta.status !== "running" &&
    detail.meta.status !== "archived" &&
    event.event_type === "error" &&
    detail.meta.last_failed_attempt_id === event.attempt_id;
  return ownsActiveAttempt || ownsCompletedAttempt || ownsFailedAttempt;
}

function activityMatchesRuntimeGeneration(
  runtime: QuickSessionRuntimeOverlay | undefined,
  event: ScopedQuickSessionActivity,
): boolean {
  return !(
    runtime?.attemptId === event.attempt_id &&
    runtime.contextGeneration !== undefined &&
    runtime.contextGeneration !== event.context_generation
  );
}

function activityCanUpdateRuntime(
  runtime: QuickSessionRuntimeOverlay | undefined,
  event: ScopedQuickSessionActivity,
  allowAttemptReplacement = false,
): boolean {
  return (
    (allowAttemptReplacement ||
      runtime?.attemptId === undefined ||
      runtime.attemptId === event.attempt_id) &&
    activityMatchesRuntimeGeneration(runtime, event)
  );
}

function hasQueuedInput(
  detail: QuickSessionDetail,
  runtime: QuickSessionRuntimeOverlay | undefined,
): runtime is QuickSessionRuntimeOverlay & { queuedInputLine: number } {
  if (
    runtime?.queuedInputLine === undefined ||
    detail.archived ||
    detail.meta.status === "archived" ||
    detail.meta.status === "error"
  ) {
    return false;
  }
  return (
    (detail.meta.processing_input_line ?? 0) < runtime.queuedInputLine &&
    (detail.meta.last_completed_input_line ?? 0) < runtime.queuedInputLine
  );
}

function runtimeFromActivity(
  detail: QuickSessionDetail,
  current: QuickSessionRuntimeOverlay | undefined,
  event: ScopedQuickSessionActivity,
): QuickSessionRuntimeOverlay {
  const queuedInputLine = hasQueuedInput(detail, current)
    ? current.queuedInputLine
    : undefined;
  return {
    status: queuedInputLine === undefined ? event.event_type : "queued",
    revision: Math.max(
      detail.meta.revision,
      current?.revision ?? 0,
      event.session_revision,
    ),
    attemptId: event.attempt_id,
    contextGeneration: event.context_generation,
    latestEvent: event,
    ...(queuedInputLine === undefined ? {} : { queuedInputLine }),
  };
}

function withoutPendingActivity(
  pending: Record<string, ScopedQuickSessionActivity>,
  sessionId: string,
): Record<string, ScopedQuickSessionActivity> {
  if (!(sessionId in pending)) return pending;
  const next = { ...pending };
  delete next[sessionId];
  return next;
}

function withoutRuntimeOverlay(
  runtimeById: Record<string, QuickSessionRuntimeOverlay>,
  sessionId: string,
): Record<string, QuickSessionRuntimeOverlay> {
  if (!(sessionId in runtimeById)) return runtimeById;
  const next = { ...runtimeById };
  delete next[sessionId];
  return next;
}

function archivedDetailFromResponse(
  detail: QuickSessionDetail,
  archived: ArchiveQuickSessionResponse,
): QuickSessionDetail {
  const archivedFrom =
    detail.meta.status === "running"
      ? detail.meta.title
        ? "active"
        : "needs_title"
      : detail.meta.status;
  return {
    ...detail,
    archived: true,
    meta: {
      ...detail.meta,
      status: "archived",
      revision: Math.max(detail.meta.revision, archived.revision),
      updated_at: archived.archived_at,
      archived_at: archived.archived_at,
      archived_from: archivedFrom,
      processing_input_line: undefined,
      processing_started_at: undefined,
      attempt_id: undefined,
    },
  };
}

function withPendingActivity(
  pending: Record<string, ScopedQuickSessionActivity>,
  event: ScopedQuickSessionActivity,
): Record<string, ScopedQuickSessionActivity> | null {
  const current = pending[event.session_id];
  if (current) {
    if (event.session_revision < current.session_revision) return null;
    if (
      event.attempt_id === current.attempt_id &&
      event.context_generation !== current.context_generation
    ) {
      return null;
    }
    if (
      event.session_revision === current.session_revision &&
      event.attempt_id !== current.attempt_id
    ) {
      return null;
    }
  }

  const next = { ...pending };
  if (!current && Object.keys(next).length >= MAX_PENDING_ACTIVITIES) {
    const oldestSessionId = Object.keys(next)[0];
    if (oldestSessionId !== undefined) delete next[oldestSessionId];
  }
  next[event.session_id] = event;
  return next;
}

function pendingAfterAppliedActivity(
  pending: Record<string, ScopedQuickSessionActivity>,
  detail: QuickSessionDetail,
  event: ScopedQuickSessionActivity,
): Record<string, ScopedQuickSessionActivity> {
  const current = pending[event.session_id];
  if (!current) return pending;
  const sameContext =
    current.attempt_id === event.attempt_id &&
    current.context_generation === event.context_generation;
  if (
    current.session_revision <= detail.meta.revision ||
    (sameContext && current.session_revision <= event.session_revision)
  ) {
    return withoutPendingActivity(pending, event.session_id);
  }
  return pending;
}

function runtimeWithQueuedInput(
  current: QuickSessionRuntimeOverlay | undefined,
  detail: QuickSessionDetail | undefined,
  inputLine: number,
  revision: number,
): QuickSessionRuntimeOverlay {
  const detailAlreadyOwnsInput =
    detail !== undefined &&
    detail.meta.revision >= revision &&
    ((detail.meta.processing_input_line ?? 0) >= inputLine ||
      (detail.meta.last_completed_input_line ?? 0) >= inputLine ||
      detail.archived ||
      detail.meta.status === "archived");
  if (detailAlreadyOwnsInput) {
    return current ?? {
      status: detail?.meta.status ?? "active",
      revision: Math.max(detail?.meta.revision ?? 0, revision),
    };
  }

  const preservesEarlierAttempt =
    detail?.meta.status === "running" &&
    (detail.meta.processing_input_line ?? 0) < inputLine;
  return {
    ...(preservesEarlierAttempt ? current : undefined),
    status: "queued",
    revision: Math.max(current?.revision ?? 0, revision),
    queuedInputLine: inputLine,
  };
}

export const useQuickSessionStore = create<QuickSessionState>((set, get) => ({
  activeSlug: null,
  items: [],
  selectedId: null,
  detailById: {},
  runtimeById: {},
  pendingActivityById: {},
  showArchived: false,
  loading: { ...EMPTY_LOADING },
  errors: { ...EMPTY_ERRORS },
  pendingCreate: null,
  pendingSend: null,

  setShowArchived: (showArchived) => set({ showArchived }),
  select: (selectedId) => set({ selectedId }),

  refreshList: async (slug) => {
    activateSlug(slug);
    const request = beginOperation("list");
    setOperation(set, "list", true, null);
    try {
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
        const items = sortQuickSessionItems(response.data!.sessions.map((item) => {
          const current = existing.get(item.id);
          return current && current.revision > item.revision ? current : item;
        }));
        return {
          items,
          loading: { ...state.loading, list: false },
          errors: { ...state.errors, list: null },
        };
      });
    } catch (error) {
      if (!operationIsCurrent(slug, "list", request)) return;
      setOperation(
        set,
        "list",
        false,
        rejectedOperationMessage(error, "Failed to list Quick Sessions"),
      );
    }
  },

  open: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("detail");
    set((state) => ({
      selectedId: id,
      loading: { ...state.loading, detail: true },
      errors: { ...state.errors, detail: null },
    }));
    try {
      const response = await client.readQuickSession(slug, id, {
        limit: QUICK_SESSION_HUB_TRANSCRIPT_LIMIT,
      });
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
    } catch (error) {
      if (!operationIsCurrent(slug, "detail", request)) return;
      setOperation(
        set,
        "detail",
        false,
        rejectedOperationMessage(error, "Failed to read Quick Session"),
      );
    }
  },

  create: async (slug, agentId, firstMessage) => {
    activateSlug(slug);
    const request = beginOperation("create");
    setOperation(set, "create", true, null);
    try {
      const currentPending = get().pendingCreate;
      const pending =
        currentPending &&
        sameCreatePayload(
          currentPending,
          slug,
          agentId,
          firstMessage,
        )
          ? currentPending
          : {
              slug,
              agentId,
              firstMessage,
              sessionId: generateQuickSessionId(),
            };
      if (pending !== currentPending) set({ pendingCreate: pending });

      const response = await client.createQuickSession(
        slug,
        agentId,
        firstMessage,
        pending.sessionId,
      );
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
      set((state) => ({
        selectedId: response.data!.session.meta.id,
        runtimeById: {
          ...state.runtimeById,
          [response.data!.session.meta.id]: runtimeWithQueuedInput(
            state.runtimeById[response.data!.session.meta.id],
            state.detailById[response.data!.session.meta.id],
            response.data!.line_number,
            response.data!.session.meta.revision,
          ),
        },
        pendingCreate:
          state.pendingCreate?.sessionId === pending.sessionId
            ? null
            : state.pendingCreate,
      }));
      setOperation(set, "create", false, null);
      return response.data.session.meta.id;
    } catch (error) {
      if (!operationIsCurrent(slug, "create", request)) return null;
      setOperation(
        set,
        "create",
        false,
        rejectedOperationMessage(error, "Failed to create Quick Session"),
      );
      return null;
    }
  },

  send: async (slug, id, body) => {
    activateSlug(slug);
    const request = beginOperation("send");
    setOperation(set, "send", true, null);
    try {
      const currentPending = get().pendingSend;
      const pending =
        currentPending && sameSendPayload(currentPending, slug, id, body)
          ? currentPending
          : {
              slug,
              sessionId: id,
              body,
              requestId: generateQuickSessionRequestId(),
            };
      if (pending !== currentPending) set({ pendingSend: pending });

      const response = await client.sendQuickSessionMessage(
        slug,
        id,
        body,
        pending.requestId,
      );
      if (!operationIsCurrent(slug, "send", request)) return false;
      if (!response.ok || !response.data) {
        setOperation(
          set,
          "send",
          false,
          response.error ?? "Failed to send Quick Session message",
        );
        return false;
      }
      const acknowledged = response.data;
      set((state) => ({
        runtimeById: {
          ...state.runtimeById,
          [id]: runtimeWithQueuedInput(
            state.runtimeById[id],
            state.detailById[id],
            acknowledged.line_number,
            acknowledged.revision,
          ),
        },
        pendingSend:
          state.pendingSend?.requestId === pending.requestId
            ? null
            : state.pendingSend,
      }));
      let detailError: string | null = null;
      try {
        const detailResponse = await client.readQuickSession(slug, id, {
          limit: QUICK_SESSION_HUB_TRANSCRIPT_LIMIT,
        });
        if (!operationIsCurrent(slug, "send", request)) return false;
        if (detailResponse.ok && detailResponse.data) {
          get().applyDetail(detailResponse.data.session);
        } else {
          detailError =
            detailResponse.error ?? "Failed to read Quick Session";
        }
      } catch (error) {
        if (!operationIsCurrent(slug, "send", request)) return false;
        detailError = rejectedOperationMessage(
          error,
          "Failed to read Quick Session",
        );
      }
      if (get().selectedId === id) {
        set((state) => ({
          errors: { ...state.errors, detail: detailError },
        }));
      }
      setOperation(set, "send", false, null);
      return true;
    } catch (error) {
      if (!operationIsCurrent(slug, "send", request)) return false;
      setOperation(
        set,
        "send",
        false,
        rejectedOperationMessage(
          error,
          "Failed to send Quick Session message",
        ),
      );
      return false;
    }
  },

  archive: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("archive");
    setOperation(set, "archive", true, null);
    try {
      const response = await client.archiveQuickSession(slug, id);
      if (!operationIsCurrent(slug, "archive", request)) return false;
      if (!response.ok || !response.data) {
        setOperation(
          set,
          "archive",
          false,
          response.error ?? "Failed to archive Quick Session",
        );
        return false;
      }
      const archived = response.data;
      set((state) => {
        const detail = state.detailById[id];
        return {
          detailById: detail
            ? {
                ...state.detailById,
                [id]: archivedDetailFromResponse(detail, archived),
              }
            : state.detailById,
          runtimeById: withoutRuntimeOverlay(state.runtimeById, id),
          pendingActivityById: withoutPendingActivity(
            state.pendingActivityById,
            id,
          ),
        };
      });
      await get().refreshList(slug);
      if (!operationIsCurrent(slug, "archive", request)) return false;
      setOperation(set, "archive", false, null);
      return true;
    } catch (error) {
      if (!operationIsCurrent(slug, "archive", request)) return false;
      setOperation(
        set,
        "archive",
        false,
        rejectedOperationMessage(error, "Failed to archive Quick Session"),
      );
      return false;
    }
  },

  unarchive: async (slug, id) => {
    activateSlug(slug);
    const request = beginOperation("archive");
    setOperation(set, "archive", true, null);
    try {
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
    } catch (error) {
      if (!operationIsCurrent(slug, "archive", request)) return false;
      setOperation(
        set,
        "archive",
        false,
        rejectedOperationMessage(error, "Failed to unarchive Quick Session"),
      );
      return false;
    }
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
      const items = sortQuickSessionItems(
        detail.archived === state.showArchived ? [...without, incoming] : without,
      );
      const previousRuntime = state.runtimeById[id];
      const pendingActivity = state.pendingActivityById[id];
      const replaysPendingActivity =
        pendingActivity !== undefined &&
        pendingActivity.session_revision <= detail.meta.revision &&
        detailOwnsActivity(detail, pendingActivity) &&
        activityCanUpdateRuntime(previousRuntime, pendingActivity, true);
      const preservesQueuedMessage = hasQueuedInput(detail, previousRuntime);
      const preservesPreviousAttempt =
        previousRuntime?.attemptId !== undefined &&
        (detail.meta.attempt_id !== undefined
          ? detail.meta.attempt_id === previousRuntime.attemptId
          : detail.meta.last_completed_attempt_id === previousRuntime.attemptId ||
            detail.meta.last_failed_attempt_id === previousRuntime.attemptId);
      const runtime: QuickSessionRuntimeOverlay = replaysPendingActivity
        ? runtimeFromActivity(detail, previousRuntime, pendingActivity)
        : preservesQueuedMessage
        ? {
            ...previousRuntime,
            revision: Math.max(previousRuntime.revision, detail.meta.revision),
          }
        : preservesPreviousAttempt
        ? {
            ...previousRuntime,
            status: detail.meta.status,
            revision: detail.meta.revision,
          }
        : {
            status: detail.meta.status,
            revision: detail.meta.revision,
            ...(detail.meta.attempt_id
              ? { attemptId: detail.meta.attempt_id }
              : {}),
          };
      const pendingIsResolved =
        pendingActivity !== undefined &&
        (replaysPendingActivity ||
          detail.archived ||
          pendingActivity.session_revision <= detail.meta.revision);
      return {
        items,
        detailById: { ...state.detailById, [id]: detail },
        runtimeById: { ...state.runtimeById, [id]: runtime },
        pendingActivityById: pendingIsResolved
          ? withoutPendingActivity(state.pendingActivityById, id)
          : state.pendingActivityById,
      };
    });
    return true;
  },

  applyActivityEvent: (event) => {
    if (!isScopedQuickSessionActivity(event)) {
      return false;
    }
    const activeSlug = get().activeSlug;
    if (
      event.workspace_id !== undefined &&
      event.workspace_id !== activeSlug
    ) {
      return false;
    }
    const detail = get().detailById[event.session_id];
    if (!detail) {
      if (activeSlug === null) return false;
      const pendingActivityById = withPendingActivity(
        get().pendingActivityById,
        event,
      );
      if (!pendingActivityById) return false;
      set({ pendingActivityById });
      return true;
    }
    const ownsActivity = detailOwnsActivity(detail, event);
    const previous = get().runtimeById[event.session_id];
    if (!ownsActivity) {
      const canBuffer =
        !detail.archived &&
        event.session_revision > detail.meta.revision;
      if (!canBuffer) return false;
      const pendingActivityById = withPendingActivity(
        get().pendingActivityById,
        event,
      );
      if (!pendingActivityById) return false;
      set({ pendingActivityById });
      return true;
    }
    if (!activityCanUpdateRuntime(previous, event)) {
      return false;
    }
    set((state) => {
      const current = state.runtimeById[event.session_id!];
      return {
        runtimeById: {
          ...state.runtimeById,
          [event.session_id]: runtimeFromActivity(detail, current, event),
        },
        pendingActivityById: pendingAfterAppliedActivity(
          state.pendingActivityById,
          detail,
          event,
        ),
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
      pendingActivityById: {},
      showArchived: false,
      loading: { ...EMPTY_LOADING },
      errors: { ...EMPTY_ERRORS },
      pendingCreate: null,
      pendingSend: null,
    });
  },
}));

onWorkspaceSwitch(() => {
  useQuickSessionStore.getState().resetForWorkspaceSwitch();
});
