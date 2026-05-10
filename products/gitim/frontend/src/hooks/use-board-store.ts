import { create } from "zustand";
import type { BoardReadResponse, BoardSummary } from "@/lib/types";

interface BoardState {
  boards: BoardSummary[];
  selectedHandler: string | null;
  selectedBoard: BoardReadResponse | null;

  setBoards: (boards: BoardSummary[]) => void;
  setSelectedHandler: (handler: string | null) => void;
  setSelectedBoard: (board: BoardReadResponse | null) => void;
  resetForWorkspaceSwitch: () => void;
}

export const useBoardStore = create<BoardState>((set) => ({
  boards: [],
  selectedHandler: null,
  selectedBoard: null,

  setBoards: (boards) =>
    set((state) => {
      const keep =
        state.selectedHandler &&
        boards.some((board) => board.handler === state.selectedHandler)
          ? state.selectedHandler
          : null;
      const selectedHandler = keep ?? boards[0]?.handler ?? null;
      const selectedBoard =
        state.selectedBoard?.handler === selectedHandler
          ? state.selectedBoard
          : null;

      return { boards, selectedHandler, selectedBoard };
    }),

  setSelectedHandler: (handler) =>
    set((state) => ({
      selectedHandler: handler,
      selectedBoard:
        state.selectedBoard?.handler === handler ? state.selectedBoard : null,
    })),

  setSelectedBoard: (board) =>
    set({
      selectedBoard: board,
      selectedHandler: board?.handler ?? null,
    }),

  resetForWorkspaceSwitch: () =>
    set({
      boards: [],
      selectedHandler: null,
      selectedBoard: null,
    }),
}));
