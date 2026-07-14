// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

import type {
  AgentActivityEvent,
  ApiResponse,
  QuickSessionDetail,
  QuickSessionListItem,
} from "../lib/types";
import { emitWorkspaceSwitch } from "../lib/workspace-lifecycle";
import { useQuickSessionStore } from "./use-quick-session-store";

const SESSION_ID = "qs-01JZZZZZZZZZZZZZZZZZZZZZZZ";
const ATTEMPT_ID = "qa-01JYYYYYYYYYYYYYYYYYYYYYYY";

const api = vi.hoisted(() => ({
  createQuickSession: vi.fn(),
  listQuickSessions: vi.fn(),
  readQuickSession: vi.fn(),
  sendQuickSessionMessage: vi.fn(),
  archiveQuickSession: vi.fn(),
  unarchiveQuickSession: vi.fn(),
}));

vi.mock("../lib/client", () => api);

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((next) => {
    resolve = next;
  });
  return { promise, resolve };
}

function detail(overrides: Partial<QuickSessionDetail["meta"]> = {}): QuickSessionDetail {
  return {
    meta: {
      id: SESSION_ID,
      title: "Investigate flakes",
      title_source: "api_set",
      agent_id: "alice",
      created_by: "lewis",
      status: "running",
      created_at: "2026-07-11T00:00:00Z",
      updated_at: "2026-07-11T00:00:03Z",
      last_message_preview: "working",
      processing_input_line: 1,
      processing_started_at: "2026-07-11T00:00:02Z",
      attempt_id: ATTEMPT_ID,
      last_human_line: 1,
      revision: 3,
      ...overrides,
    },
    entries: [
      {
        line_number: 1,
        point_to: 0,
        author: "lewis",
        timestamp: "20260711T000000Z",
        body: "Investigate flakes",
      },
    ],
    archived: false,
  };
}

function listItem(session = detail()): QuickSessionListItem {
  return {
    id: session.meta.id,
    title: session.meta.title ?? null,
    agent_id: session.meta.agent_id,
    created_by: session.meta.created_by,
    status: session.meta.status,
    updated_at: session.meta.updated_at,
    last_message_preview: session.meta.last_message_preview,
    revision: session.meta.revision,
    archived: session.archived,
    ref: `session:${session.meta.id}`,
  };
}

function ok<T>(data: T): ApiResponse<T> {
  return { ok: true, data };
}

function quickEvent(
  overrides: Partial<AgentActivityEvent> = {},
): AgentActivityEvent {
  return {
    agent_id: "alice",
    event_type: "thinking",
    detail: "checking the failure",
    timestamp: "2026-07-11T00:00:04Z",
    scope: "quick_session",
    session_id: SESSION_ID,
    session_revision: 3,
    attempt_id: ATTEMPT_ID,
    context_generation: 2,
    ...overrides,
  };
}

describe("useQuickSessionStore", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useQuickSessionStore.getState().resetForWorkspaceSwitch();
  });

  it("loads, opens, creates, sends, and archives through the scoped client", async () => {
    const initial = detail({ status: "active", attempt_id: undefined });
    api.listQuickSessions.mockResolvedValue(
      ok({ sessions: [listItem(initial)] }),
    );
    api.readQuickSession.mockResolvedValue(ok({ session: initial }));
    api.createQuickSession.mockResolvedValue(
      ok({ session: initial, line_number: 1, ref: `session:${SESSION_ID}` }),
    );
    api.sendQuickSessionMessage.mockResolvedValue(
      ok({
        session_id: SESSION_ID,
        line_number: 2,
        status: "active",
        revision: 4,
        ref: `session:${SESSION_ID}:L000002`,
      }),
    );
    api.archiveQuickSession.mockResolvedValue(
      ok({
        session_id: SESSION_ID,
        status: "archived",
        revision: 5,
        archived_at: "2026-07-11T00:00:05Z",
      }),
    );

    await useQuickSessionStore.getState().refreshList("alpha");
    expect(useQuickSessionStore.getState().items).toEqual([listItem(initial)]);

    await useQuickSessionStore.getState().open("alpha", SESSION_ID);
    expect(useQuickSessionStore.getState().selectedId).toBe(SESSION_ID);
    expect(useQuickSessionStore.getState().detailById[SESSION_ID]).toEqual(initial);

    await useQuickSessionStore
      .getState()
      .create("alpha", "alice", "Investigate flakes");
    expect(api.createQuickSession).toHaveBeenCalledWith(
      "alpha",
      "alice",
      "Investigate flakes",
    );

    await useQuickSessionStore.getState().send("alpha", SESSION_ID, "continue");
    expect(api.sendQuickSessionMessage).toHaveBeenCalledWith(
      "alpha",
      SESSION_ID,
      "continue",
    );

    api.listQuickSessions.mockResolvedValue(ok({ sessions: [] }));
    await useQuickSessionStore.getState().archive("alpha", SESSION_ID);
    expect(api.archiveQuickSession).toHaveBeenCalledWith("alpha", SESSION_ID);
    expect(useQuickSessionStore.getState().items).toEqual([]);
    expect(useQuickSessionStore.getState().errors.archive).toBeNull();
  });

  it("discards async results after a workspace switch", async () => {
    const pending = deferred<ApiResponse<{ sessions: QuickSessionListItem[] }>>();
    api.listQuickSessions.mockReturnValue(pending.promise);

    const refresh = useQuickSessionStore.getState().refreshList("alpha");
    emitWorkspaceSwitch();
    pending.resolve(ok({ sessions: [listItem()] }));
    await refresh;

    expect(useQuickSessionStore.getState().items).toEqual([]);
    expect(useQuickSessionStore.getState().loading.list).toBe(false);
  });

  it("keeps newer metadata when an older detail response arrives", async () => {
    const newer = detail({ revision: 8, status: "active", attempt_id: undefined });
    useQuickSessionStore.getState().applyDetail(newer);
    const pending = deferred<ApiResponse<{ session: QuickSessionDetail }>>();
    api.readQuickSession.mockReturnValue(pending.promise);

    const open = useQuickSessionStore.getState().open("alpha", SESSION_ID);
    pending.resolve(ok({ session: detail({ revision: 7 }) }));
    await open;

    expect(
      useQuickSessionStore.getState().detailById[SESSION_ID]?.meta.revision,
    ).toBe(8);
  });

  it("routes only the active attempt and current generation", () => {
    useQuickSessionStore.getState().applyDetail(detail());

    expect(
      useQuickSessionStore.getState().applyActivityEvent(quickEvent()),
    ).toBe(true);
    expect(
      useQuickSessionStore
        .getState()
        .runtimeById[SESSION_ID]?.latestEvent?.detail,
    ).toBe("checking the failure");

    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          attempt_id: "qa-01JXXXXXXXXXXXXXXXXXXXXXXX",
          detail: "stale attempt",
        }),
      ),
    ).toBe(false);
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({ context_generation: 1, detail: "stale generation" }),
      ),
    ).toBe(false);
  });

  it("preserves same-attempt progress across metadata revision bumps", () => {
    useQuickSessionStore.getState().applyDetail(detail({ revision: 3 }));
    useQuickSessionStore.getState().applyActivityEvent(quickEvent());

    useQuickSessionStore.getState().applyDetail(
      detail({ revision: 5, title: "A better title" }),
    );
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          session_revision: 3,
          event_type: "tool_use",
          detail: "running tests",
        }),
      ),
    ).toBe(true);
    expect(
      useQuickSessionStore
        .getState()
        .runtimeById[SESSION_ID]?.latestEvent?.detail,
    ).toBe("running tests");
    expect(useQuickSessionStore.getState().items[0]?.revision).toBe(5);
  });

  it("refreshes only Quick Session changes and targets the selected detail", async () => {
    useQuickSessionStore.setState({ selectedId: SESSION_ID });
    api.listQuickSessions.mockResolvedValue(ok({ sessions: [listItem()] }));
    api.readQuickSession.mockResolvedValue(ok({ session: detail() }));

    await useQuickSessionStore.getState().refreshFromPoll("alpha", [
      { channel: "general", kind: "new_messages", entries: [] },
    ]);
    expect(api.listQuickSessions).not.toHaveBeenCalled();

    await useQuickSessionStore.getState().refreshFromPoll("alpha", [
      { channel: SESSION_ID, kind: "quick_session_meta" },
      {
        channel: "qs-01JAAAAAAAAAAAAAAAAAAAAAAA",
        kind: "quick_session_thread",
      },
    ]);
    expect(api.listQuickSessions).toHaveBeenCalledTimes(1);
    expect(api.readQuickSession).toHaveBeenCalledTimes(1);
    expect(api.readQuickSession).toHaveBeenCalledWith("alpha", SESSION_ID);
  });
});
