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
const OTHER_SESSION_ID = "qs-01JYYYYYYYYYYYYYYYYYYYYYYY";
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
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((next, fail) => {
    resolve = next;
    reject = fail;
  });
  return { promise, resolve, reject };
}

function detail(overrides: Partial<QuickSessionDetail["meta"]> = {}): QuickSessionDetail {
  return {
    meta: {
      id: SESSION_ID,
      title: "Investigate flakes",
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
    expect(api.readQuickSession).toHaveBeenLastCalledWith(
      "alpha",
      SESSION_ID,
      { limit: 50 },
    );
    expect(useQuickSessionStore.getState().selectedId).toBe(SESSION_ID);
    expect(useQuickSessionStore.getState().detailById[SESSION_ID]).toEqual(initial);

    await useQuickSessionStore
      .getState()
      .create("alpha", "alice", "Investigate flakes");
    expect(api.createQuickSession).toHaveBeenCalledWith(
      "alpha",
      "alice",
      "Investigate flakes",
      expect.stringMatching(/^qs-[0-9A-HJKMNP-TV-Z]{26}$/),
    );

    await useQuickSessionStore.getState().send("alpha", SESSION_ID, "continue");
    expect(api.sendQuickSessionMessage).toHaveBeenCalledWith(
      "alpha",
      SESSION_ID,
      "continue",
      expect.stringMatching(/^[0-9A-HJKMNP-TV-Z]{26}$/),
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

  it("reuses create and send ids after a committed response is lost", async () => {
    let createdId = "";
    api.createQuickSession
      .mockRejectedValueOnce(new Error("create response lost"))
      .mockImplementationOnce(
        async (
          _slug: string,
          _agentId: string,
          _firstMessage: string,
          sessionId: string,
        ) => {
          createdId = sessionId;
          return ok({
            session: detail({ id: sessionId }),
            line_number: 1,
            ref: `session:${sessionId}`,
          });
        },
      );

    await expect(
      useQuickSessionStore
        .getState()
        .create("alpha", "alice", "Investigate flakes"),
    ).resolves.toBeNull();
    const pendingSessionId = api.createQuickSession.mock.calls[0]?.[3];
    expect(pendingSessionId).toMatch(/^qs-[0-9A-HJKMNP-TV-Z]{26}$/);

    await expect(
      useQuickSessionStore
        .getState()
        .create("alpha", "alice", "Investigate flakes"),
    ).resolves.toBe(pendingSessionId);
    expect(createdId).toBe(pendingSessionId);
    expect(useQuickSessionStore.getState().pendingCreate).toBeNull();

    api.sendQuickSessionMessage
      .mockRejectedValueOnce(new Error("send response lost"))
      .mockImplementationOnce(
        async (
          _slug: string,
          sessionId: string,
          _body: string,
          requestId: string,
        ) =>
          ok({
            session_id: sessionId,
            line_number: 2,
            status: "active",
            revision: 4,
            ref: `session:${sessionId}:L000002`,
            request_id: requestId,
          }),
      );
    api.readQuickSession.mockResolvedValue(
      ok({ session: detail({ id: createdId, status: "active" }) }),
    );

    await expect(
      useQuickSessionStore.getState().send("alpha", createdId, "continue"),
    ).resolves.toBe(false);
    const pendingRequestId = api.sendQuickSessionMessage.mock.calls[0]?.[3];
    expect(pendingRequestId).toMatch(/^[0-9A-HJKMNP-TV-Z]{26}$/);
    await expect(
      useQuickSessionStore.getState().send("alpha", createdId, "continue"),
    ).resolves.toBe(true);
    expect(api.sendQuickSessionMessage.mock.calls[1]?.[3]).toBe(
      pendingRequestId,
    );
    expect(useQuickSessionStore.getState().pendingSend).toBeNull();
  });

  it("does not reuse pending ids when the operation payload changes", async () => {
    api.createQuickSession.mockRejectedValue(new Error("offline"));
    await useQuickSessionStore
      .getState()
      .create("alpha", "alice", "first payload");
    await useQuickSessionStore
      .getState()
      .create("alpha", "alice", "second payload");

    expect(api.createQuickSession.mock.calls[0]?.[3]).toMatch(
      /^qs-[0-9A-HJKMNP-TV-Z]{26}$/,
    );
    expect(api.createQuickSession.mock.calls[1]?.[3]).not.toBe(
      api.createQuickSession.mock.calls[0]?.[3],
    );

    api.sendQuickSessionMessage.mockRejectedValue(new Error("offline"));
    await useQuickSessionStore.getState().send("alpha", SESSION_ID, "first");
    await useQuickSessionStore.getState().send("alpha", SESSION_ID, "second");
    expect(api.sendQuickSessionMessage.mock.calls[0]?.[3]).toMatch(
      /^[0-9A-HJKMNP-TV-Z]{26}$/,
    );
    expect(api.sendQuickSessionMessage.mock.calls[1]?.[3]).not.toBe(
      api.sendQuickSessionMessage.mock.calls[0]?.[3],
    );
    expect(useQuickSessionStore.getState().pendingCreate).not.toBeNull();
    expect(useQuickSessionStore.getState().pendingSend).not.toBeNull();

    emitWorkspaceSwitch();
    expect(useQuickSessionStore.getState().pendingCreate).toBeNull();
    expect(useQuickSessionStore.getState().pendingSend).toBeNull();
  });

  it("refreshes a late send without restoring its previous selection", async () => {
    const pending = deferred<ApiResponse>();
    const sentDetail = detail({
      id: SESSION_ID,
      status: "active",
      attempt_id: undefined,
      revision: 4,
    });
    const otherDetail = detail({
      id: OTHER_SESSION_ID,
      status: "active",
      attempt_id: undefined,
      revision: 2,
    });
    api.sendQuickSessionMessage.mockReturnValue(pending.promise);
    api.readQuickSession.mockImplementation(
      async (_slug: string, id: string) =>
        ok({ session: id === SESSION_ID ? sentDetail : otherDetail }),
    );
    useQuickSessionStore.getState().applyDetail(detail({ status: "active" }));
    useQuickSessionStore.getState().select(SESSION_ID);

    const sending = useQuickSessionStore
      .getState()
      .send("alpha", SESSION_ID, "continue");
    await useQuickSessionStore.getState().open("alpha", OTHER_SESSION_ID);
    expect(useQuickSessionStore.getState().selectedId).toBe(OTHER_SESSION_ID);

    pending.resolve(
      ok({
        session_id: SESSION_ID,
        line_number: 2,
        status: "active",
        revision: 4,
        ref: `session:${SESSION_ID}:L000002`,
      }),
    );
    await expect(sending).resolves.toBe(true);

    expect(useQuickSessionStore.getState().selectedId).toBe(OTHER_SESSION_ID);
    expect(
      useQuickSessionStore.getState().detailById[SESSION_ID]?.meta.revision,
    ).toBe(4);
    expect(api.readQuickSession).toHaveBeenLastCalledWith(
      "alpha",
      SESSION_ID,
      { limit: 50 },
    );
  });

  it("discards a send follow-up read after the workspace changes", async () => {
    const pendingRead = deferred<
      ApiResponse<{ session: QuickSessionDetail }>
    >();
    api.sendQuickSessionMessage.mockResolvedValue(
      ok({
        session_id: SESSION_ID,
        line_number: 2,
        status: "active",
        revision: 4,
        ref: `session:${SESSION_ID}:L000002`,
      }),
    );
    api.readQuickSession.mockReturnValue(pendingRead.promise);
    useQuickSessionStore.getState().applyDetail(detail({ status: "active" }));
    useQuickSessionStore.getState().select(SESSION_ID);

    const sending = useQuickSessionStore
      .getState()
      .send("alpha", SESSION_ID, "continue");
    await vi.waitFor(() => {
      expect(api.readQuickSession).toHaveBeenCalledWith(
        "alpha",
        SESSION_ID,
        { limit: 50 },
      );
    });
    emitWorkspaceSwitch();
    pendingRead.resolve(
      ok({
        session: detail({
          status: "active",
          attempt_id: undefined,
          revision: 4,
        }),
      }),
    );

    await expect(sending).resolves.toBe(false);
    expect(useQuickSessionStore.getState().activeSlug).toBeNull();
    expect(useQuickSessionStore.getState().detailById).toEqual({});
  });

  it("turns transport rejections into per-operation errors", async () => {
    const cases = [
      {
        operation: "list" as const,
        mock: api.listQuickSessions,
        run: () => useQuickSessionStore.getState().refreshList("alpha"),
      },
      {
        operation: "detail" as const,
        mock: api.readQuickSession,
        run: () => useQuickSessionStore.getState().open("alpha", SESSION_ID),
      },
      {
        operation: "create" as const,
        mock: api.createQuickSession,
        run: () =>
          useQuickSessionStore
            .getState()
            .create("alpha", "alice", "Investigate flakes"),
      },
      {
        operation: "send" as const,
        mock: api.sendQuickSessionMessage,
        run: () =>
          useQuickSessionStore.getState().send("alpha", SESSION_ID, "continue"),
      },
      {
        operation: "archive" as const,
        mock: api.archiveQuickSession,
        run: () => useQuickSessionStore.getState().archive("alpha", SESSION_ID),
      },
      {
        operation: "archive" as const,
        mock: api.unarchiveQuickSession,
        run: () => useQuickSessionStore.getState().unarchive("alpha", SESSION_ID),
      },
    ];

    for (const [index, testCase] of cases.entries()) {
      useQuickSessionStore.getState().resetForWorkspaceSwitch();
      testCase.mock.mockRejectedValueOnce(
        new Error(`transport failed ${index}`),
      );
      await testCase.run();
      expect(
        useQuickSessionStore.getState().loading[testCase.operation],
      ).toBe(false);
      expect(
        useQuickSessionStore.getState().errors[testCase.operation],
      ).toBe(`transport failed ${index}`);
    }
  });

  it("discards a transport rejection after the workspace changes", async () => {
    const pending = deferred<ApiResponse<{ sessions: QuickSessionListItem[] }>>();
    api.listQuickSessions.mockReturnValue(pending.promise);
    const refresh = useQuickSessionStore.getState().refreshList("alpha");

    emitWorkspaceSwitch();
    pending.reject(new Error("late failure"));
    await expect(refresh).resolves.toBeUndefined();

    expect(useQuickSessionStore.getState().activeSlug).toBeNull();
    expect(useQuickSessionStore.getState().errors.list).toBeNull();
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

  it("accepts completion events when poll clears the active claim first", () => {
    useQuickSessionStore.getState().applyDetail(detail({ revision: 3 }));
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({ context_generation: 4 }),
      ),
    ).toBe(true);

    useQuickSessionStore.getState().applyDetail(
      detail({
        status: "active",
        attempt_id: undefined,
        processing_input_line: undefined,
        processing_started_at: undefined,
        last_completed_attempt_id: ATTEMPT_ID,
        last_completed_input_line: 1,
        last_completed_line: 2,
        last_failed_attempt_id: "qa-01JXXXXXXXXXXXXXXXXXXXXXXX",
        revision: 6,
      }),
    );

    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          event_type: "usage",
          detail: JSON.stringify({
            session_id: "provider-quick",
            used_percent: 42,
            source: "provider_reported",
            updated_at: "2026-07-11T00:00:05Z",
          }),
          session_revision: 3,
          context_generation: 4,
        }),
      ),
    ).toBe(true);
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          event_type: "done",
          detail: "done",
          session_revision: 3,
          context_generation: 4,
        }),
      ),
    ).toBe(true);

    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          event_type: "done",
          attempt_id: "qa-01JXXXXXXXXXXXXXXXXXXXXXXX",
          context_generation: 4,
        }),
      ),
    ).toBe(false);
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({ event_type: "done", context_generation: 3 }),
      ),
    ).toBe(false);
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          event_type: "error",
          attempt_id: "qa-01JXXXXXXXXXXXXXXXXXXXXXXX",
          context_generation: 4,
        }),
      ),
    ).toBe(false);
  });

  it("accepts a terminal error when poll records the failed attempt first", () => {
    useQuickSessionStore.getState().applyDetail(detail({ revision: 3 }));
    useQuickSessionStore.getState().applyActivityEvent(
      quickEvent({ context_generation: 7 }),
    );
    useQuickSessionStore.getState().applyDetail(
      detail({
        status: "error",
        attempt_id: undefined,
        processing_input_line: undefined,
        processing_started_at: undefined,
        last_failed_attempt_id: ATTEMPT_ID,
        error: "provider failed",
        revision: 4,
      }),
    );

    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({
          event_type: "error",
          detail: "provider failed",
          context_generation: 7,
        }),
      ),
    ).toBe(true);
    expect(
      useQuickSessionStore.getState().applyActivityEvent(
        quickEvent({ event_type: "done", context_generation: 7 }),
      ),
    ).toBe(false);
  });

  it("keeps list items ordered by newest update and then id", () => {
    useQuickSessionStore.getState().applyDetail(
      detail({ id: SESSION_ID, updated_at: "2026-07-11T00:00:03Z" }),
    );
    useQuickSessionStore.getState().applyDetail(
      detail({ id: OTHER_SESSION_ID, updated_at: "2026-07-11T00:00:04Z" }),
    );
    expect(useQuickSessionStore.getState().items.map((item) => item.id)).toEqual([
      OTHER_SESSION_ID,
      SESSION_ID,
    ]);

    useQuickSessionStore.getState().applyDetail(
      detail({ id: SESSION_ID, updated_at: "2026-07-11T00:00:04Z", revision: 4 }),
    );
    expect(useQuickSessionStore.getState().items.map((item) => item.id)).toEqual([
      OTHER_SESSION_ID,
      SESSION_ID,
    ]);
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
    expect(api.readQuickSession).toHaveBeenCalledWith("alpha", SESSION_ID, {
      limit: 50,
    });
  });
});
