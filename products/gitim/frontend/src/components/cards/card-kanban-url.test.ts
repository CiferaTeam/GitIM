// @vitest-environment jsdom
// Tests for the URL round-trip helpers in card-kanban.tsx.
// These are pure functions; no rendering required.

import { describe, expect, it, vi } from "vitest";

// card-kanban.tsx imports modules that rely on the browser environment.
// Mock the heavy deps so we can import the pure URL helpers in isolation.
vi.mock("react-router", () => ({
  useSearchParams: vi.fn(() => [new URLSearchParams(), vi.fn()]),
}));
vi.mock("@/hooks/use-card-store", () => ({
  useCardStore: vi.fn(),
  selectAllLabels: vi.fn(),
  selectFilteredCards: vi.fn(() => []),
  sortByUpdatedDesc: vi.fn((x: unknown[]) => x),
}));
vi.mock("@/hooks/use-chat-store", () => ({ useChatStore: vi.fn() }));
vi.mock("@/hooks/use-workspace-store", () => ({ useWorkspaceStore: vi.fn() }));
vi.mock("@/lib/client", () => ({ listCards: vi.fn() }));
vi.mock("./card-filter-bar", () => ({
  CardFilterBar: () => null,
  EMPTY_CARD_FILTER: {
    channels: [],
    labels: [],
    assignee: null,
    mineOnly: false,
    project: null,
  },
}));
vi.mock("./card-kanban-column", () => ({ CardKanbanColumn: () => null }));
vi.mock("./card-create-dialog", () => ({ CardCreateDialog: () => null }));
vi.mock("@/components/mobile/mobile-card-list", () => ({
  MobileCardList: () => null,
}));
vi.mock("@/components/ui/button", () => ({
  Button: ({ children }: { children: unknown }) => children,
}));
vi.mock("lucide-react", () => ({ Plus: () => null }));
vi.mock("sonner", () => ({ toast: { error: vi.fn() } }));

import {
  readFilterFromURL,
  writeFilterToURL,
} from "./card-kanban";
import type { CardFilterState } from "./card-filter-bar";

const EMPTY: CardFilterState = {
  channels: [],
  labels: [],
  assignee: null,
  mineOnly: false,
  project: null,
};

describe("writeFilterToURL / readFilterFromURL — project round-trip", () => {
  it("round-trips a specific project slug", () => {
    const filter: CardFilterState = { ...EMPTY, project: "design" };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("design");
    const back = readFilterFromURL(params);
    expect(back.project).toBe("design");
  });

  it("round-trips the __unassigned__ sentinel", () => {
    const filter: CardFilterState = { ...EMPTY, project: "__unassigned__" };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("__unassigned__");
    const back = readFilterFromURL(params);
    expect(back.project).toBe("__unassigned__");
  });

  it("omits project param when project is null (All)", () => {
    const filter: CardFilterState = { ...EMPTY, project: null };
    const params = writeFilterToURL(filter);
    expect(params.has("project")).toBe(false);
  });

  it("reads null when project param is absent", () => {
    const params = new URLSearchParams();
    const back = readFilterFromURL(params);
    expect(back.project).toBeNull();
  });

  it("preserves other filter fields alongside project", () => {
    const filter: CardFilterState = {
      channels: ["dev"],
      labels: ["bug"],
      assignee: null,
      mineOnly: false,
      project: "infra",
    };
    const params = writeFilterToURL(filter);
    expect(params.get("project")).toBe("infra");
    expect(params.getAll("channel")).toEqual(["dev"]);
    expect(params.getAll("label")).toEqual(["bug"]);
    const back = readFilterFromURL(params);
    expect(back.project).toBe("infra");
    expect(back.channels).toEqual(["dev"]);
    expect(back.labels).toEqual(["bug"]);
  });

  it("mineOnly round-trips with project filter present", () => {
    const filter: CardFilterState = {
      ...EMPTY,
      mineOnly: true,
      project: "design",
    };
    const params = writeFilterToURL(filter);
    expect(params.get("assignee")).toBe("__me__");
    expect(params.get("project")).toBe("design");
    const back = readFilterFromURL(params);
    expect(back.mineOnly).toBe(true);
    expect(back.assignee).toBeNull();
    expect(back.project).toBe("design");
  });
});
