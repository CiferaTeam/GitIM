import { create } from "zustand";
import { persist } from "zustand/middleware";
import {
  DEFAULT_DISPLAY_TIMEZONE,
  normalizeDisplayTimezone,
  type DisplayTimezone,
} from "@/lib/timezone";

interface TimezoneState {
  timezone: DisplayTimezone;
  setTimezone: (timezone: DisplayTimezone) => void;
}

export const useTimezoneStore = create<TimezoneState>()(
  persist(
    (set) => ({
      timezone: DEFAULT_DISPLAY_TIMEZONE,
      setTimezone: (timezone) =>
        set({ timezone: normalizeDisplayTimezone(timezone) }),
    }),
    {
      name: "gitim-timezone",
      partialize: (state) => ({ timezone: state.timezone }),
      onRehydrateStorage: () => (state) => {
        if (state) {
          state.timezone = normalizeDisplayTimezone(state.timezone);
        }
      },
    },
  ),
);
