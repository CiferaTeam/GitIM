import { useEffect, useState } from "react";
import { fetchStats, type StatsDay } from "../lib/cell-api";

// Refresh cadence for the community-pulse indicator. 5 min feels live to
// anyone with the tab open, while keeping request volume trivial compared to
// runtime polling. On fetch failure we keep the last good snapshot rather
// than clearing — a blip shouldn't hide the sparkline mid-session.
const STATS_REFRESH_MS = 5 * 60 * 1000;

export function useStats(): StatsDay[] | null {
  const [days, setDays] = useState<StatsDay[] | null>(null);

  useEffect(() => {
    let cancelled = false;

    async function tick() {
      const r = await fetchStats();
      if (!cancelled && r) setDays(r);
    }

    void tick();
    const handle = setInterval(tick, STATS_REFRESH_MS);
    return () => {
      cancelled = true;
      clearInterval(handle);
    };
  }, []);

  return days;
}
