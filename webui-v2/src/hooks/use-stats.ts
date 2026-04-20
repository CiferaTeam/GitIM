import { useEffect, useState } from "react";
import { fetchStats, type StatsDay } from "../lib/cell-api";

// Daily-granularity data — one fetch per mount is enough. No polling; the
// indicator is a "community pulse" signal, not a live dashboard.
export function useStats(): StatsDay[] | null {
  const [days, setDays] = useState<StatsDay[] | null>(null);

  useEffect(() => {
    let cancelled = false;
    fetchStats().then((r) => {
      if (!cancelled) setDays(r);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return days;
}
